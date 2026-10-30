import XCTest
@testable import SidecastKit

final class FramerTests: XCTestCase {
    func testByteByByteDelivery() throws {
        let messages: [ClientMessage] = [.pairStart, .ping(42), .frameAck(seq: 9), .keyframeRequest]
        let wire = messages.map { $0.encoded() }.reduce(Data(), +)
        let parser = FrameParser()
        var decoded: [ClientMessage] = []
        for byte in wire {
            parser.append(Data([byte]))
            while let frame = try parser.nextFrame() {
                decoded.append(try XCTUnwrap(try ClientMessage.decode(frame)))
            }
        }
        XCTAssertEqual(decoded, messages)
        XCTAssertEqual(parser.bufferedByteCount, 0)
    }

    func testSplitAcrossArbitraryChunks() throws {
        let big = VideoFrame(seq: 1, isKeyframe: false, ptsMicros: 5, data: Data(repeating: 7, count: 300_000))
        let wire = ServerMessage.videoFrame(big).encoded() + ServerMessage.pong(3).encoded()
        let parser = FrameParser()
        var frames: [Frame] = []
        var i = 0
        let sizes = [1, 4, 5, 1000, 65_536, 3]
        var k = 0
        while i < wire.count {
            let n = min(sizes[k % sizes.count], wire.count - i)
            parser.append(wire.subdata(in: i..<(i + n)))
            frames += try parser.drain()
            i += n
            k += 1
        }
        XCTAssertEqual(frames.count, 2)
        XCTAssertEqual(try ServerMessage.decode(frames[0]), .videoFrame(big))
        XCTAssertEqual(try ServerMessage.decode(frames[1]), .pong(3))
    }

    func testIncompleteHeaderAndPayloadWait() throws {
        let parser = FrameParser()
        parser.append(Data([0x13, 0x00, 0x00]))
        XCTAssertNil(try parser.nextFrame())
        parser.append(Data([0x00, 0x08, 0x00]))
        XCTAssertNil(try parser.nextFrame())
        parser.append(Data(repeating: 0, count: 7))
        XCTAssertEqual(try parser.nextFrame(), Frame(.ping, payload: Data(repeating: 0, count: 8)))
    }

    func testOversizeLengthIsProtocolError() {
        let parser = FrameParser()
        // 16 MiB + 1
        parser.append(Data([0x85, 0x01, 0x00, 0x00, 0x01]))
        XCTAssertThrowsError(try parser.nextFrame()) { error in
            XCTAssertEqual(error as? ProtocolError, .payloadTooLarge(16 * 1024 * 1024 + 1))
        }
    }

    func testExactly16MiBIsAllowed() throws {
        let parser = FrameParser()
        parser.append(Data([0x7F, 0x01, 0x00, 0x00, 0x00]))
        XCTAssertNil(try parser.nextFrame())   // waits for the payload instead of failing
    }

    func testUnknownTypesAreSkipped() throws {
        let unknown = Frame(type: 0x42, payload: Data([1, 2, 3])).encoded()
        let serverTypeFromClient = Frame(.bye, payload: Data("{}".utf8)).encoded()
        let parser = FrameParser()
        parser.append(unknown + serverTypeFromClient + ClientMessage.ping(1).encoded())
        let frames = try parser.drain()
        XCTAssertEqual(frames.count, 3)
        XCTAssertNil(try ClientMessage.decode(frames[0]))
        XCTAssertNil(try ClientMessage.decode(frames[1]))
        XCTAssertEqual(try ClientMessage.decode(frames[2]), .ping(1))
        XCTAssertNil(try ServerMessage.decode(Frame(type: 0x99)))
    }

    func testEmptyPayload() throws {
        let wire = ClientMessage.pairStart.encoded()
        XCTAssertEqual(wire, Data([0x03, 0, 0, 0, 0]))
        let parser = FrameParser()
        parser.append(wire)
        XCTAssertEqual(try ClientMessage.decode(try XCTUnwrap(try parser.nextFrame())), .pairStart)
    }

    func testTruncatedBinaryPayloadsThrow() {
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.frameAck, payload: Data([0, 1]))))
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.ping, payload: Data([0]))))
        XCTAssertThrowsError(try ServerMessage.decode(Frame(.videoFrame, payload: Data(count: 12))))
        // TOUCH declaring 2 pointers but carrying one.
        let one = TouchEvent(eventTimeMs: 1, action: .down, actionPointerId: 0,
                             pointers: [TouchPointer(id: 0, x: 0, y: 0)])
        var payload = one.encodedPayload()
        payload[6] = 2
        XCTAssertThrowsError(try ClientMessage.decode(Frame(.touch, payload: payload)))
    }
}
