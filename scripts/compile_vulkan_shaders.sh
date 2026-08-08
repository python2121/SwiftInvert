#!/usr/bin/env bash
# Regenerate the checked-in SPIR-V binaries from NegPipeline.comp.
# Run inside the swiftdev distrobox (needs glslangValidator from
# glslang-tools). The .spv files are bundled as SwiftPM resources so the
# build itself needs no GLSL compiler — rerun this after ANY shader edit
# and commit the .spv files together with the .comp source.
set -euo pipefail
cd "$(dirname "$0")/../Sources/VulkanRenderKit/Shaders"

declare -A KERNELS=(
    [KERNEL_NORMALIZE]=normalize
    [KERNEL_CURVE]=print_curve
    [KERNEL_COLORPOP]=color_pop
    [KERNEL_HISTOGRAM]=histogram
    [KERNEL_ENCODE_F]=encode_f
    [KERNEL_ENCODE_U8]=encode_u8
)

for def in "${!KERNELS[@]}"; do
    out="${KERNELS[$def]}.spv"
    glslangValidator -V -S comp -D"$def" NegPipeline.comp -o "$out" > /dev/null
    echo "compiled $out"
done
