import Foundation
import Testing

/// Locates Tests/Fixtures (dumped from NegPy by scripts/dump_fixtures.py).
enum Fixtures {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // NegativeKitTests
        .deletingLastPathComponent()  // Tests
        .appendingPathComponent("Fixtures")

    static func json(_ relativePath: String) throws -> [String: Any] {
        let data = try Data(contentsOf: root.appendingPathComponent(relativePath))
        return try #require(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    /// Little-endian float32 blob dumped by numpy `tofile`.
    static func floats(_ relativePath: String) throws -> [Float] {
        let data = try Data(contentsOf: root.appendingPathComponent(relativePath))
        return data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
    }
}

/// #expect with tolerance, XCTAssertEqual(_:accuracy:)-style.
func expectClose(
    _ got: Double, _ expected: Double, accuracy: Double, _ label: @autoclosure () -> String = "",
    sourceLocation: SourceLocation = #_sourceLocation
) {
    #expect(
        abs(got - expected) <= accuracy,
        "\(label()) got \(got), expected \(expected) ± \(accuracy)",
        sourceLocation: sourceLocation)
}

@Suite struct FixtureSmoke {
    @Test func fixturesPresent() throws {
        let manifest = try Fixtures.json("synthetic64/manifest.json")
        #expect(manifest["input"] != nil)
        let input = try Fixtures.floats("synthetic64/input.bin")
        #expect(input.count == 64 * 64 * 3)
    }
}
