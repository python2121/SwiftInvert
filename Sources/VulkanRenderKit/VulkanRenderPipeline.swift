import CVulkan
import Foundation
import NegativeKit

public enum VulkanError: Error, CustomStringConvertible {
    case noDevice
    case vk(String, Int32)
    case resource(String)

    public var description: String {
        switch self {
        case .noDevice: return "No Vulkan compute device available"
        case .vk(let op, let code): return "Vulkan \(op) failed (VkResult \(code))"
        case .resource(let m): return "Vulkan resource allocation failed: \(m)"
        }
    }
}

@discardableResult
func vkCheck(_ result: VkResult, _ op: String) throws -> VkResult {
    guard result == VK_SUCCESS else { throw VulkanError.vk(op, result.rawValue) }
    return result
}

/// A host-visible VkBuffer with its memory, persistently mapped. Holds the
/// context strongly so the VkDevice outlives every buffer — property release
/// order in the pipeline is unspecified, and unmapping against a destroyed
/// device is exactly the crash negcli's teardown found.
final class DeviceBuffer {
    let context: VulkanContext
    var device: VkDevice { context.device }
    var buffer: VkBuffer?
    var memory: VkDeviceMemory?
    var mapped: UnsafeMutableRawPointer?
    let size: Int

    /// `hostReadback`: the CPU will READ this buffer's contents. On discrete
    /// GPUs the DEVICE_LOCAL|HOST_VISIBLE (BAR) types are write-combined —
    /// CPU writes stream fine, CPU reads are uncached and ~1000× slower (the
    /// first bench: 3.9 s/frame, all of it memcpy-ing the output out of BAR).
    /// Readback targets therefore want HOST_CACHED system memory; everything
    /// else wants BAR so the GPU passes run at VRAM speed.
    init(context: VulkanContext, size: Int, usage: VkBufferUsageFlags, hostReadback: Bool = false) throws {
        self.context = context
        self.size = size

        var info = VkBufferCreateInfo()
        info.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO
        info.size = VkDeviceSize(size)
        info.usage = usage
        info.sharingMode = VK_SHARING_MODE_EXCLUSIVE
        try vkCheck(vkCreateBuffer(device, &info, nil, &buffer), "vkCreateBuffer")

        var req = VkMemoryRequirements()
        vkGetBufferMemoryRequirements(device, buffer, &req)

        var alloc = VkMemoryAllocateInfo()
        alloc.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO
        alloc.allocationSize = req.size
        alloc.memoryTypeIndex = try context.hostVisibleMemoryType(
            bits: req.memoryTypeBits, cached: hostReadback)
        try vkCheck(vkAllocateMemory(device, &alloc, nil, &memory), "vkAllocateMemory")
        try vkCheck(vkBindBufferMemory(device, buffer, memory, 0), "vkBindBufferMemory")

        var ptr: UnsafeMutableRawPointer?
        try vkCheck(vkMapMemory(device, memory, 0, VK_WHOLE_SIZE, 0, &ptr), "vkMapMemory")
        mapped = ptr
    }

    deinit {
        if memory != nil { vkUnmapMemory(device, memory) }
        if buffer != nil { vkDestroyBuffer(device, buffer, nil) }
        if memory != nil { vkFreeMemory(device, memory, nil) }
    }
}

/// Instance/device plumbing shared by the pipeline and its buffers.
final class VulkanContext {
    var instance: VkInstance!
    var physicalDevice: VkPhysicalDevice!
    var device: VkDevice!
    var queue: VkQueue!
    var queueFamily: UInt32 = 0
    var memoryProperties = VkPhysicalDeviceMemoryProperties()
    var deviceName = "unknown"

    init() throws {
        var appInfo = VkApplicationInfo()
        appInfo.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO
        appInfo.apiVersion = 4194304  // VK_API_VERSION_1_0 (1 << 22)

        var instInfo = VkInstanceCreateInfo()
        instInfo.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO
        try withUnsafePointer(to: appInfo) { appPtr in
            instInfo.pApplicationInfo = appPtr
            try vkCheck(vkCreateInstance(&instInfo, nil, &instance), "vkCreateInstance")
        }

        // Enumerate and pick: discrete > integrated > anything with compute.
        // NEGCLI_VK_CPU=1 forces the CPU implementation (llvmpipe) — useful
        // to exercise the pipeline where no GPU is visible.
        var count: UInt32 = 0
        try vkCheck(vkEnumeratePhysicalDevices(instance, &count, nil), "vkEnumeratePhysicalDevices")
        guard count > 0 else { throw VulkanError.noDevice }
        var devices = [VkPhysicalDevice?](repeating: nil, count: Int(count))
        try vkCheck(
            vkEnumeratePhysicalDevices(instance, &count, &devices), "vkEnumeratePhysicalDevices")

        let forceCPU = ProcessInfo.processInfo.environment["NEGCLI_VK_CPU"] == "1"
        var best: (VkPhysicalDevice, UInt32, Int, String)? = nil  // device, family, score, name
        for dev in devices.compactMap({ $0 }) {
            var props = VkPhysicalDeviceProperties()
            vkGetPhysicalDeviceProperties(dev, &props)
            let name = withUnsafeBytes(of: props.deviceName) { raw in
                String(cString: raw.bindMemory(to: CChar.self).baseAddress!)
            }
            var famCount: UInt32 = 0
            vkGetPhysicalDeviceQueueFamilyProperties(dev, &famCount, nil)
            var families = [VkQueueFamilyProperties](
                repeating: VkQueueFamilyProperties(), count: Int(famCount))
            vkGetPhysicalDeviceQueueFamilyProperties(dev, &famCount, &families)
            guard
                let family = families.firstIndex(where: {
                    $0.queueFlags & VkQueueFlags(VK_QUEUE_COMPUTE_BIT.rawValue) != 0
                })
            else { continue }
            let isCPU = props.deviceType == VK_PHYSICAL_DEVICE_TYPE_CPU
            if forceCPU != isCPU { continue }
            let score: Int
            switch props.deviceType {
            case VK_PHYSICAL_DEVICE_TYPE_DISCRETE_GPU: score = 3
            case VK_PHYSICAL_DEVICE_TYPE_INTEGRATED_GPU: score = 2
            default: score = 1
            }
            if best == nil || score > best!.2 {
                best = (dev, UInt32(family), score, name)
            }
        }
        guard let (dev, family, _, name) = best else { throw VulkanError.noDevice }
        physicalDevice = dev
        queueFamily = family
        deviceName = name
        vkGetPhysicalDeviceMemoryProperties(dev, &memoryProperties)

        var priority: Float = 1.0
        var queueInfo = VkDeviceQueueCreateInfo()
        queueInfo.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO
        queueInfo.queueFamilyIndex = family
        queueInfo.queueCount = 1
        try withUnsafePointer(to: priority) { prioPtr in
            queueInfo.pQueuePriorities = prioPtr
            var devInfo = VkDeviceCreateInfo()
            devInfo.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO
            devInfo.queueCreateInfoCount = 1
            try withUnsafePointer(to: queueInfo) { qPtr in
                devInfo.pQueueCreateInfos = qPtr
                try vkCheck(vkCreateDevice(dev, &devInfo, nil, &device), "vkCreateDevice")
            }
        }
        vkGetDeviceQueue(device, family, 0, &queue)
        _ = priority  // keep alive through the call above
    }

    /// Memory type index, HOST_VISIBLE|HOST_COHERENT always. `cached: false`
    /// prefers +DEVICE_LOCAL (BAR: VRAM-speed for the GPU, streamable CPU
    /// writes). `cached: true` prefers +HOST_CACHED and avoids DEVICE_LOCAL
    /// (readable CPU memory — BAR reads are uncached write-combined).
    func hostVisibleMemoryType(bits: UInt32, cached: Bool) throws -> UInt32 {
        let base =
            VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT.rawValue
            | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT.rawValue
        let local = VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT.rawValue
        let hostCached = VK_MEMORY_PROPERTY_HOST_CACHED_BIT.rawValue
        let types: [VkMemoryType] = withUnsafeBytes(of: memoryProperties.memoryTypes) { raw in
            Array(raw.bindMemory(to: VkMemoryType.self).prefix(Int(memoryProperties.memoryTypeCount)))
        }
        func find(_ want: UInt32, avoid: UInt32 = 0) -> UInt32? {
            for (i, t) in types.enumerated() where bits & (1 << UInt32(i)) != 0 {
                if t.propertyFlags & want == want && t.propertyFlags & avoid == 0 {
                    return UInt32(i)
                }
            }
            return nil
        }
        let pick =
            cached
            ? find(base | hostCached, avoid: local) ?? find(base | hostCached) ?? find(base, avoid: local) ?? find(base)
            : find(base | local) ?? find(base)
        guard let pick else { throw VulkanError.resource("no host-visible memory type") }
        return pick
    }

    deinit {
        if device != nil { vkDestroyDevice(device, nil) }
        if instance != nil { vkDestroyInstance(instance, nil) }
    }
}

/// The Vulkan compute mirror of MetalRenderKit's RenderPipeline: same pass
/// chain (normalize → curve → [colorPop] → [histogram] + encode), same
/// uniform packing (the shared ShaderTypes.swift), same Result shapes.
/// Buffers hold interleaved RGB float32 — RGBImage's own layout — so upload
/// and readback are memcpys. Serialized by a lock exactly like the Metal
/// pipeline (shared intermediates).
public final class VulkanRenderPipeline: @unchecked Sendable {
    let context: VulkanContext
    public var deviceName: String { context.deviceName }

    // One pipeline per kernel; two descriptor-set layouts (SSBO/SSBO/UBO for
    // the value passes, SSBO/SSBO/SSBO for the levels-consuming passes).
    var layoutUBO: VkDescriptorSetLayout?
    var layoutSSBO: VkDescriptorSetLayout?
    var pipeLayoutUBO: VkPipelineLayout?
    var pipeLayoutSSBO: VkPipelineLayout?
    var pipelines: [String: VkPipeline] = [:]
    var descriptorPool: VkDescriptorPool?
    var commandPool: VkCommandPool?
    var commandBuffer: VkCommandBuffer?
    var fence: VkFence?

    // Uniform/aux buffers (small, persistent).
    var normUBO: DeviceBuffer!
    var curveUBO: DeviceBuffer!
    var levelsSSBO: DeviceBuffer!
    var histSSBO: DeviceBuffer!

    /// Reused intermediates per size, mirroring the Metal cache discipline.
    private struct SizeKey: Hashable {
        let w: Int
        let h: Int
    }
    private var intermediates: [SizeKey: (normalized: DeviceBuffer, linear: DeviceBuffer, encoded: DeviceBuffer?)] = [:]
    private var display8: [SizeKey: DeviceBuffer] = [:]
    private let maxCachedPixels = 4_000_000
    private let renderLock = NSLock()

    public final class SourceBuffer {
        let buffer: DeviceBuffer
        public let width: Int
        public let height: Int
        init(buffer: DeviceBuffer, width: Int, height: Int) {
            self.buffer = buffer
            self.width = width
            self.height = height
        }
    }

    public init() throws {
        context = try VulkanContext()
        try makeLayouts()
        try makePipelines()
        try makePools()
        let ssbo = VkBufferUsageFlags(VK_BUFFER_USAGE_STORAGE_BUFFER_BIT.rawValue)
        let ubo = VkBufferUsageFlags(VK_BUFFER_USAGE_UNIFORM_BUFFER_BIT.rawValue)
        let xfer = VkBufferUsageFlags(VK_BUFFER_USAGE_TRANSFER_DST_BIT.rawValue)
        normUBO = try DeviceBuffer(context: context, size: 48, usage: ubo)
        curveUBO = try DeviceBuffer(context: context, size: 272, usage: ubo)
        levelsSSBO = try DeviceBuffer(context: context, size: 51 * 4, usage: ssbo)
        histSSBO = try DeviceBuffer(context: context, size: 1024 * 4, usage: ssbo | xfer, hostReadback: true)
    }

    // MARK: - Setup

    private func makeLayouts() throws {
        func layout(types: [VkDescriptorType]) throws -> VkDescriptorSetLayout? {
            var bindings = types.enumerated().map { i, t -> VkDescriptorSetLayoutBinding in
                var b = VkDescriptorSetLayoutBinding()
                b.binding = UInt32(i)
                b.descriptorType = t
                b.descriptorCount = 1
                b.stageFlags = VkShaderStageFlags(VK_SHADER_STAGE_COMPUTE_BIT.rawValue)
                return b
            }
            var info = VkDescriptorSetLayoutCreateInfo()
            info.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_LAYOUT_CREATE_INFO
            info.bindingCount = UInt32(bindings.count)
            var result: VkDescriptorSetLayout?
            try bindings.withUnsafeBufferPointer { buf in
                info.pBindings = buf.baseAddress
                try vkCheck(
                    vkCreateDescriptorSetLayout(context.device, &info, nil, &result),
                    "vkCreateDescriptorSetLayout")
            }
            return result
        }
        layoutUBO = try layout(types: [
            VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,
            VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER,
        ])
        layoutSSBO = try layout(types: [
            VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,
            VK_DESCRIPTOR_TYPE_STORAGE_BUFFER,
        ])

        func pipeLayout(_ setLayout: VkDescriptorSetLayout?) throws -> VkPipelineLayout? {
            var push = VkPushConstantRange()
            push.stageFlags = VkShaderStageFlags(VK_SHADER_STAGE_COMPUTE_BIT.rawValue)
            push.offset = 0
            push.size = 4  // uint n
            var info = VkPipelineLayoutCreateInfo()
            info.sType = VK_STRUCTURE_TYPE_PIPELINE_LAYOUT_CREATE_INFO
            info.setLayoutCount = 1
            info.pushConstantRangeCount = 1
            var result: VkPipelineLayout?
            var layoutVar = setLayout
            try withUnsafePointer(to: layoutVar) { lPtr in
                info.pSetLayouts = lPtr
                try withUnsafePointer(to: push) { pPtr in
                    info.pPushConstantRanges = pPtr
                    try vkCheck(
                        vkCreatePipelineLayout(context.device, &info, nil, &result),
                        "vkCreatePipelineLayout")
                }
            }
            _ = layoutVar
            return result
        }
        pipeLayoutUBO = try pipeLayout(layoutUBO)
        pipeLayoutSSBO = try pipeLayout(layoutSSBO)
    }

    private func loadSPIRV(_ name: String) throws -> [UInt32] {
        guard
            let url = Bundle.module.url(forResource: name, withExtension: "spv", subdirectory: "Shaders")
                ?? Bundle.module.url(forResource: name, withExtension: "spv"),
            let data = try? Data(contentsOf: url)
        else { throw VulkanError.resource("\(name).spv not found in bundle") }
        return data.withUnsafeBytes { raw in
            Array(raw.bindMemory(to: UInt32.self))
        }
    }

    private func makePipelines() throws {
        // (kernel spv name, uses the UBO layout)
        let kernels: [(String, Bool)] = [
            ("normalize", true), ("print_curve", true), ("color_pop", true),
            ("histogram", false), ("encode_f", false), ("encode_u8", false),
        ]
        for (name, usesUBO) in kernels {
            let code = try loadSPIRV(name)
            var moduleInfo = VkShaderModuleCreateInfo()
            moduleInfo.sType = VK_STRUCTURE_TYPE_SHADER_MODULE_CREATE_INFO
            moduleInfo.codeSize = code.count * 4
            var module: VkShaderModule?
            try code.withUnsafeBufferPointer { buf in
                moduleInfo.pCode = buf.baseAddress
                try vkCheck(
                    vkCreateShaderModule(context.device, &moduleInfo, nil, &module),
                    "vkCreateShaderModule")
            }
            defer { vkDestroyShaderModule(context.device, module, nil) }

            var pipeline: VkPipeline?
            try "main".withCString { entry in
                var stage = VkPipelineShaderStageCreateInfo()
                stage.sType = VK_STRUCTURE_TYPE_PIPELINE_SHADER_STAGE_CREATE_INFO
                stage.stage = VK_SHADER_STAGE_COMPUTE_BIT
                stage.module = module
                stage.pName = entry
                var info = VkComputePipelineCreateInfo()
                info.sType = VK_STRUCTURE_TYPE_COMPUTE_PIPELINE_CREATE_INFO
                info.stage = stage
                info.layout = usesUBO ? pipeLayoutUBO : pipeLayoutSSBO
                try vkCheck(
                    vkCreateComputePipelines(context.device, nil, 1, &info, nil, &pipeline),
                    "vkCreateComputePipelines(\(name))")
            }
            pipelines[name] = pipeline
        }
    }

    private func makePools() throws {
        var poolSizes = [
            VkDescriptorPoolSize(type: VK_DESCRIPTOR_TYPE_STORAGE_BUFFER, descriptorCount: 24),
            VkDescriptorPoolSize(type: VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER, descriptorCount: 8),
        ]
        var poolInfo = VkDescriptorPoolCreateInfo()
        poolInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_POOL_CREATE_INFO
        poolInfo.maxSets = 8
        poolInfo.poolSizeCount = UInt32(poolSizes.count)
        try poolSizes.withUnsafeBufferPointer { buf in
            poolInfo.pPoolSizes = buf.baseAddress
            try vkCheck(
                vkCreateDescriptorPool(context.device, &poolInfo, nil, &descriptorPool),
                "vkCreateDescriptorPool")
        }

        var cmdPoolInfo = VkCommandPoolCreateInfo()
        cmdPoolInfo.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO
        cmdPoolInfo.flags = VkCommandPoolCreateFlags(
            VK_COMMAND_POOL_CREATE_RESET_COMMAND_BUFFER_BIT.rawValue)
        cmdPoolInfo.queueFamilyIndex = context.queueFamily
        try vkCheck(
            vkCreateCommandPool(context.device, &cmdPoolInfo, nil, &commandPool),
            "vkCreateCommandPool")

        var allocInfo = VkCommandBufferAllocateInfo()
        allocInfo.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO
        allocInfo.commandPool = commandPool
        allocInfo.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY
        allocInfo.commandBufferCount = 1
        try vkCheck(
            vkAllocateCommandBuffers(context.device, &allocInfo, &commandBuffer),
            "vkAllocateCommandBuffers")

        var fenceInfo = VkFenceCreateInfo()
        fenceInfo.sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO
        try vkCheck(vkCreateFence(context.device, &fenceInfo, nil, &fence), "vkCreateFence")
    }

    deinit {
        if context.device != nil {
            vkDeviceWaitIdle(context.device)
            intermediates.removeAll()
            display8.removeAll()
            for (_, p) in pipelines { vkDestroyPipeline(context.device, p, nil) }
            if fence != nil { vkDestroyFence(context.device, fence, nil) }
            if commandPool != nil { vkDestroyCommandPool(context.device, commandPool, nil) }
            if descriptorPool != nil { vkDestroyDescriptorPool(context.device, descriptorPool, nil) }
            if pipeLayoutUBO != nil { vkDestroyPipelineLayout(context.device, pipeLayoutUBO, nil) }
            if pipeLayoutSSBO != nil { vkDestroyPipelineLayout(context.device, pipeLayoutSSBO, nil) }
            if layoutUBO != nil { vkDestroyDescriptorSetLayout(context.device, layoutUBO, nil) }
            if layoutSSBO != nil { vkDestroyDescriptorSetLayout(context.device, layoutSSBO, nil) }
        }
    }

    // MARK: - Buffers

    private func storageBuffer(size: Int, hostReadback: Bool = false) throws -> DeviceBuffer {
        try DeviceBuffer(
            context: context, size: size,
            usage: VkBufferUsageFlags(VK_BUFFER_USAGE_STORAGE_BUFFER_BIT.rawValue),
            hostReadback: hostReadback)
    }

    public func upload(_ image: RGBImage) throws -> SourceBuffer {
        let buf = try storageBuffer(size: image.pixels.count * 4)
        image.pixels.withUnsafeBufferPointer { src in
            buf.mapped!.copyMemory(from: src.baseAddress!, byteCount: src.count * 4)
        }
        return SourceBuffer(buffer: buf, width: image.width, height: image.height)
    }

    private func readback(_ buf: DeviceBuffer, width: Int, height: Int) -> RGBImage {
        RGBImage(width: width, height: height) { dst in
            dst.baseAddress!.update(
                from: buf.mapped!.assumingMemoryBound(to: Float.self),
                count: width * height * 3)
        }
    }

    private func intermediatesFor(width w: Int, height h: Int, needEncoded: Bool) throws
        -> (normalized: DeviceBuffer, linear: DeviceBuffer, encoded: DeviceBuffer?)
    {
        let key = SizeKey(w: w, h: h)
        let bytes = w * h * 3 * 4
        var set: (normalized: DeviceBuffer, linear: DeviceBuffer, encoded: DeviceBuffer?)
        if let cached = intermediates[key] {
            set = cached
        } else {
            set = (normalized: try storageBuffer(size: bytes), linear: try storageBuffer(size: bytes), encoded: nil)
        }
        if needEncoded && set.encoded == nil {
            set.encoded = try storageBuffer(size: bytes, hostReadback: true)
        }
        if w * h <= maxCachedPixels {
            if intermediates[key] == nil && intermediates.count >= 4 {
                intermediates.removeAll()
            }
            intermediates[key] = set
        }
        return set
    }

    // MARK: - Recording helpers

    private func writeDescriptorSet(
        layout: VkDescriptorSetLayout?, buffers: [(DeviceBuffer, VkDescriptorType)]
    ) throws -> VkDescriptorSet? {
        var setLayout = layout
        var allocInfo = VkDescriptorSetAllocateInfo()
        allocInfo.sType = VK_STRUCTURE_TYPE_DESCRIPTOR_SET_ALLOCATE_INFO
        allocInfo.descriptorPool = descriptorPool
        allocInfo.descriptorSetCount = 1
        var set: VkDescriptorSet?
        try withUnsafePointer(to: setLayout) { lPtr in
            allocInfo.pSetLayouts = lPtr
            try vkCheck(
                vkAllocateDescriptorSets(context.device, &allocInfo, &set),
                "vkAllocateDescriptorSets")
        }
        _ = setLayout

        var infos = buffers.map { b, _ in
            VkDescriptorBufferInfo(buffer: b.buffer, offset: 0, range: VK_WHOLE_SIZE)
        }
        infos.withUnsafeBufferPointer { infoBuf in
            var writes = buffers.enumerated().map { i, pair -> VkWriteDescriptorSet in
                var w = VkWriteDescriptorSet()
                w.sType = VK_STRUCTURE_TYPE_WRITE_DESCRIPTOR_SET
                w.dstSet = set
                w.dstBinding = UInt32(i)
                w.descriptorCount = 1
                w.descriptorType = pair.1
                w.pBufferInfo = infoBuf.baseAddress! + i
                return w
            }
            writes.withUnsafeBufferPointer { wBuf in
                vkUpdateDescriptorSets(context.device, UInt32(wBuf.count), wBuf.baseAddress, 0, nil)
            }
        }
        return set
    }

    private func bindAndDispatch(
        _ cmd: VkCommandBuffer?, kernel: String, usesUBO: Bool, set: VkDescriptorSet?, n: UInt32
    ) {
        let pipeLayout = usesUBO ? pipeLayoutUBO : pipeLayoutSSBO
        vkCmdBindPipeline(cmd, VK_PIPELINE_BIND_POINT_COMPUTE, pipelines[kernel])
        var setVar = set
        withUnsafePointer(to: setVar) { sPtr in
            vkCmdBindDescriptorSets(
                cmd, VK_PIPELINE_BIND_POINT_COMPUTE, pipeLayout, 0, 1, sPtr, 0, nil)
        }
        _ = setVar
        var count = n
        vkCmdPushConstants(
            cmd, pipeLayout, VkShaderStageFlags(VK_SHADER_STAGE_COMPUTE_BIT.rawValue), 0, 4, &count)
        vkCmdDispatch(cmd, (n + 255) / 256, 1, 1)
    }

    private func computeBarrier(_ cmd: VkCommandBuffer?) {
        var barrier = VkMemoryBarrier()
        barrier.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER
        barrier.srcAccessMask = VkAccessFlags(VK_ACCESS_SHADER_WRITE_BIT.rawValue)
        barrier.dstAccessMask = VkAccessFlags(VK_ACCESS_SHADER_READ_BIT.rawValue)
        vkCmdPipelineBarrier(
            cmd, VkPipelineStageFlags(VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT.rawValue),
            VkPipelineStageFlags(VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT.rawValue), 0,
            1, &barrier, 0, nil, 0, nil)
    }

    // MARK: - Render

    public struct Result: Sendable {
        public let encoded: RGBImage
        public let linear: RGBImage?
        public let histogram: [UInt32]
    }

    public struct DisplayResult: Sendable {
        public let rgba: [UInt8]
        public let width: Int
        public let height: Int
        public let histogram: [UInt32]
    }

    private func colorPopActive(_ params: RenderParams) -> Bool {
        params.vibrance != 1.0 || params.saturation != 1.0 || params.skinProtection > 0
            || params.hueTrim != 0
            || params.bandHues != .zero || params.bandSaturations != SIMD4(repeating: 1.0)
    }

    /// Shared pass chain; `encodeKernel` picks the float or packed-u8 encode
    /// and `encodeTarget` receives it.
    private func encodeAndRun(
        source: SourceBuffer, params: RenderParams, computeHistogram: Bool,
        encodeKernel: String, encodeTarget: DeviceBuffer,
        normalized: DeviceBuffer, linear: DeviceBuffer
    ) throws -> DeviceBuffer {
        let n = UInt32(source.width * source.height)

        // Pack uniforms — same builder as Metal, memcpy'd (std140 == C here).
        var normU = UniformsBuilder.normUniforms(params)
        withUnsafeBytes(of: &normU) { normUBO.mapped!.copyMemory(from: $0.baseAddress!, byteCount: 48) }
        var curveU = UniformsBuilder.curveUniforms(params)
        withUnsafeBytes(of: &curveU) { curveUBO.mapped!.copyMemory(from: $0.baseAddress!, byteCount: 272) }
        let levels = UniformsBuilder.levelsBuffer(params)
        levels.withUnsafeBufferPointer {
            levelsSSBO.mapped!.copyMemory(from: $0.baseAddress!, byteCount: 51 * 4)
        }

        try vkCheck(
            vkResetDescriptorPool(context.device, descriptorPool, 0), "vkResetDescriptorPool")
        try vkCheck(vkResetCommandBuffer(commandBuffer, 0), "vkResetCommandBuffer")

        var begin = VkCommandBufferBeginInfo()
        begin.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO
        begin.flags = VkCommandBufferUsageFlags(VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT.rawValue)
        try vkCheck(vkBeginCommandBuffer(commandBuffer, &begin), "vkBeginCommandBuffer")

        let cmd = commandBuffer
        let ssbo = VK_DESCRIPTOR_TYPE_STORAGE_BUFFER
        let ubo = VK_DESCRIPTOR_TYPE_UNIFORM_BUFFER

        if computeHistogram {
            vkCmdFillBuffer(cmd, histSSBO.buffer, 0, VK_WHOLE_SIZE, 0)
            var barrier = VkMemoryBarrier()
            barrier.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER
            barrier.srcAccessMask = VkAccessFlags(VK_ACCESS_TRANSFER_WRITE_BIT.rawValue)
            barrier.dstAccessMask = VkAccessFlags(VK_ACCESS_SHADER_WRITE_BIT.rawValue | VK_ACCESS_SHADER_READ_BIT.rawValue)
            vkCmdPipelineBarrier(
                cmd, VkPipelineStageFlags(VK_PIPELINE_STAGE_TRANSFER_BIT.rawValue),
                VkPipelineStageFlags(VK_PIPELINE_STAGE_COMPUTE_SHADER_BIT.rawValue), 0,
                1, &barrier, 0, nil, 0, nil)
        }

        let normSet = try writeDescriptorSet(
            layout: layoutUBO,
            buffers: [(source.buffer, ssbo), (normalized, ssbo), (normUBO, ubo)])
        bindAndDispatch(cmd, kernel: "normalize", usesUBO: true, set: normSet, n: n)
        computeBarrier(cmd)

        let curveSet = try writeDescriptorSet(
            layout: layoutUBO, buffers: [(normalized, ssbo), (linear, ssbo), (curveUBO, ubo)])
        bindAndDispatch(cmd, kernel: "print_curve", usesUBO: true, set: curveSet, n: n)
        computeBarrier(cmd)

        // Color pop writes into `normalized` (already consumed), which then
        // becomes the content buffer — same trick as the Metal chain.
        var content = linear
        if colorPopActive(params) {
            let popSet = try writeDescriptorSet(
                layout: layoutUBO, buffers: [(linear, ssbo), (normalized, ssbo), (curveUBO, ubo)])
            bindAndDispatch(cmd, kernel: "color_pop", usesUBO: true, set: popSet, n: n)
            computeBarrier(cmd)
            content = normalized
        }

        if computeHistogram {
            let histSet = try writeDescriptorSet(
                layout: layoutSSBO, buffers: [(content, ssbo), (histSSBO, ssbo), (levelsSSBO, ssbo)])
            bindAndDispatch(cmd, kernel: "histogram", usesUBO: false, set: histSet, n: n)
        }

        let encodeSet = try writeDescriptorSet(
            layout: layoutSSBO, buffers: [(content, ssbo), (encodeTarget, ssbo), (levelsSSBO, ssbo)])
        bindAndDispatch(cmd, kernel: encodeKernel, usesUBO: false, set: encodeSet, n: n)

        try vkCheck(vkEndCommandBuffer(cmd), "vkEndCommandBuffer")

        var submit = VkSubmitInfo()
        submit.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO
        submit.commandBufferCount = 1
        var cmdVar = commandBuffer
        try withUnsafePointer(to: cmdVar) { cPtr in
            submit.pCommandBuffers = cPtr
            try vkCheck(vkQueueSubmit(context.queue, 1, &submit, fence), "vkQueueSubmit")
        }
        _ = cmdVar
        var fenceVar = fence
        try withUnsafePointer(to: fenceVar) { fPtr in
            try vkCheck(
                vkWaitForFences(context.device, 1, fPtr, VK_TRUE, 10_000_000_000),
                "vkWaitForFences")
            _ = vkResetFences(context.device, 1, fPtr)
        }
        _ = fenceVar
        return content
    }

    private func readHistogram() -> [UInt32] {
        let ptr = histSSBO.mapped!.assumingMemoryBound(to: UInt32.self)
        return Array(UnsafeBufferPointer(start: ptr, count: 1024))
    }

    public func render(
        source: SourceBuffer, params: RenderParams, computeHistogram: Bool = true,
        wantLinear: Bool = false
    ) throws -> Result {
        renderLock.lock()
        defer { renderLock.unlock() }
        let w = source.width, h = source.height
        let (normalized, linear, encodedOpt) = try intermediatesFor(width: w, height: h, needEncoded: true)
        let content = try encodeAndRun(
            source: source, params: params, computeHistogram: computeHistogram,
            encodeKernel: "encode_f", encodeTarget: encodedOpt!,
            normalized: normalized, linear: linear)
        return Result(
            encoded: readback(encodedOpt!, width: w, height: h),
            linear: wantLinear ? readback(content, width: w, height: h) : nil,
            histogram: readHistogram())
    }

    public func render(
        image: RGBImage, params: RenderParams, computeHistogram: Bool = true
    ) throws -> (encoded: RGBImage, histogram: [UInt32]) {
        let source = try upload(image)
        let result = try render(source: source, params: params, computeHistogram: computeHistogram)
        return (result.encoded, result.histogram)
    }

    public func renderDisplay(
        source: SourceBuffer, params: RenderParams, computeHistogram: Bool = true
    ) throws -> DisplayResult {
        renderLock.lock()
        defer { renderLock.unlock() }
        let w = source.width, h = source.height
        let (normalized, linear, _) = try intermediatesFor(width: w, height: h, needEncoded: false)

        let key = SizeKey(w: w, h: h)
        let encoded8: DeviceBuffer
        if let cached = display8[key] {
            encoded8 = cached
        } else {
            encoded8 = try storageBuffer(size: w * h * 4, hostReadback: true)
            if w * h <= maxCachedPixels {
                if display8.count >= 4 { display8.removeAll() }
                display8[key] = encoded8
            }
        }

        _ = try encodeAndRun(
            source: source, params: params, computeHistogram: computeHistogram,
            encodeKernel: "encode_u8", encodeTarget: encoded8,
            normalized: normalized, linear: linear)

        let rgba = [UInt8](unsafeUninitializedCapacity: w * h * 4) { buf, count in
            buf.baseAddress!.update(
                from: encoded8.mapped!.assumingMemoryBound(to: UInt8.self), count: w * h * 4)
            count = w * h * 4
        }
        return DisplayResult(rgba: rgba, width: w, height: h, histogram: readHistogram())
    }
}
