import XCTest
@testable import SidecastKit

final class AnnexBTests: XCTestCase {
    func testLengthPrefixedToAnnexBWithParameterSets() {
        let avcc = Data([0, 0, 0, 2, 0x26, 0x01, 0, 0, 0, 3, 0x02, 0xAA, 0xBB])
        let vps = Data([0x40, 0x01]), sps = Data([0x42, 0x01]), pps = Data([0x44, 0x01])
        let out = AnnexB.fromLengthPrefixed(avcc, parameterSets: [vps, sps, pps])
        XCTAssertEqual(out, Data([0, 0, 0, 1, 0x40, 0x01, 0, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 1, 0x44, 0x01,
                                  0, 0, 0, 1, 0x26, 0x01, 0, 0, 0, 1, 0x02, 0xAA, 0xBB]))
    }

    func testCorruptLengthIsRejected() {
        XCTAssertNil(AnnexB.fromLengthPrefixed(Data([0, 0, 0, 9, 1, 2])))
        XCTAssertNil(AnnexB.fromLengthPrefixed(Data([0, 0])))
        XCTAssertEqual(AnnexB.fromLengthPrefixed(Data()), Data())
    }

    func testSplitHandlesThreeAndFourByteStartCodes() {
        let stream = Data([0, 0, 0, 1, 0x40, 0x01, 0, 0, 1, 0x42, 0x01, 0, 0, 0, 1, 0x26, 0xFF])
        let units = AnnexB.split(stream)
        XCTAssertEqual(units, [Data([0x40, 0x01]), Data([0x42, 0x01]), Data([0x26, 0xFF])])
        XCTAssertEqual(units.compactMap(AnnexB.hevcNALType), [32, 33, 19])
        XCTAssertEqual(AnnexB.h264NALType(Data([0x67])), 7)
    }

    func testRoundTrip() {
        let units = [Data([0x40, 1, 2]), Data([0x26, 9, 9, 9])]
        let lp = AnnexB.lengthPrefixed(units)
        XCTAssertEqual(AnnexB.split(AnnexB.fromLengthPrefixed(lp)!), units)
    }
}
