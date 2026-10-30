import Foundation

// sidecast-probe: a scripted fake phone for end-to-end testing of the Mac server.

setvbuf(stdout, nil, _IOLBF, 0)

let probeOptions: ProbeOptions
do {
    probeOptions = try ProbeOptions.parse(CommandLine.arguments)
} catch {
    FileHandle.standardError.write(Data("\(error)\n".utf8))
    exit(2)
}

let client = ProbeClient(options: probeOptions) { code in
    exit(code)
}
client.start()
// Hard stop in case the server never answers.
DispatchQueue.main.asyncAfter(deadline: .now() + probeOptions.seconds + 30) {
    print("probe: RESULT FAIL (watchdog)")
    exit(1)
}
dispatchMain()
