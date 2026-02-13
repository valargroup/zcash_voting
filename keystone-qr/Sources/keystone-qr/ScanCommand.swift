import ArgumentParser
import Foundation

struct Scan: ParsableCommand {
    static let configuration = CommandConfiguration(
        abstract: "Scan QR code(s) from camera or screen"
    )

    @Flag(name: .long, help: "Capture from webcam")
    var camera = false

    @Flag(name: .long, help: "Capture from screen region")
    var screen = false

    func run() throws {
        print("Scan mode coming soon - camera: \(camera), screen: \(screen)")
    }
}
