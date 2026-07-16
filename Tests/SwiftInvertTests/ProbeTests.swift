import Testing

@testable import SwiftInvert

@Suite struct ExecutableImportProbe {
    @Test func canReachAppTypes() {
        let options = ExportOptions()
        #expect(options.format == .jpeg)
    }
}
