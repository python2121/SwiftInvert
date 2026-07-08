import Foundation
import XCTest

/// Locates Tests/Fixtures (dumped from NegPy by scripts/dump_fixtures.py).
enum Fixtures {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // NegativeKitTests
        .deletingLastPathComponent()  // Tests
        .appendingPathComponent("Fixtures")

    static func json(_ relativePath: String) throws -> [String: Any] {
        let data = try Data(contentsOf: root.appendingPathComponent(relativePath))
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    /// Little-endian float32 blob dumped by numpy `tofile`.
    static func floats(_ relativePath: String) throws -> [Float] {
        let data = try Data(contentsOf: root.appendingPathComponent(relativePath))
        return data.withUnsafeBytes { Array($0.bindMemory(to: Float.self)) }
    }
}

final class FixtureSmokeTests: XCTestCase {
    func testFixturesPresent() throws {
        let manifest = try Fixtures.json("synthetic64/manifest.json")
        XCTAssertNotNil(manifest["input"])
        let input = try Fixtures.floats("synthetic64/input.bin")
        XCTAssertEqual(input.count, 64 * 64 * 3)
    }
}
