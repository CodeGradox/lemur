// lemur-dictate, which lemur's dictation runs: it listens to the microphone
// and transcribes on this Mac with SpeechAnalyzer (macOS 26). Each time the
// transcript changes it writes all of it as a JSON line:
//
//     {"finalized": "Words that stay.", "volatile": " and a first guess"}
//
// {"listening": true} comes once the microphone is on, and {"status": "…"}
// before that says what it's doing, like downloading a language. Closing
// its stdin stops it: what's left is finalized, {"done": true} follows and
// it exits. {"error": "…"} comes before it exits with a failure.
//
// --locale picks the language (en-US without one); --file reads the speech
// from an audio file instead of the microphone, for tests.

import AVFoundation
import Foundation
import Speech

@main
struct Dictate {
    static func main() async {
        do {
            try await run(CommandLine.arguments.dropFirst())
        } catch {
            emit(["error": message(error)])
            exit(1)
        }
    }
}

struct Failure: Error {
    let text: String
    init(_ text: String) { self.text = text }
}

func message(_ error: Error) -> String {
    (error as? Failure)?.text ?? error.localizedDescription
}

func emit(_ object: [String: Any]) {
    var line = try! JSONSerialization.data(withJSONObject: object)
    line.append(0x0a)
    FileHandle.standardOutput.write(line)
}

func run(_ arguments: ArraySlice<String>) async throws {
    var localeID = "en-US"
    var file: URL?
    var rest = arguments.makeIterator()
    while let argument = rest.next() {
        switch argument {
        case "--locale": localeID = rest.next() ?? localeID
        case "--file": file = rest.next().map { URL(fileURLWithPath: $0) }
        default: throw Failure("unknown argument \(argument)")
        }
    }
    let transcript = Transcript()
    let (module, locale, results) = try await transcriber(
        for: Locale(identifier: localeID), into: transcript)
    // The language's model is only there for apps that reserve it.
    _ = try await AssetInventory.reserve(locale: locale)
    if await AssetInventory.status(forModules: [module]) != .installed,
        let request = try await AssetInventory.assetInstallationRequest(supporting: [module])
    {
        emit(["status": "downloading \(localeID) speech recognition…"])
        try await request.downloadAndInstall()
    }
    guard let format = await SpeechAnalyzer.bestAvailableAudioFormat(compatibleWith: [module]) else {
        throw Failure("no audio format suits \(localeID) speech recognition")
    }
    let converter = Converter(to: format)
    let (input, feed) = AsyncStream<AnalyzerInput>.makeStream()
    let analyzer = SpeechAnalyzer(modules: [module])
    try await analyzer.start(inputSequence: input)

    if let file {
        let audio = try AVAudioFile(forReading: file)
        // Reading at the end fails rather than reading nothing.
        while audio.framePosition < audio.length {
            let buffer = AVAudioPCMBuffer(pcmFormat: audio.processingFormat, frameCapacity: 4096)!
            try audio.read(into: buffer)
            feed.yield(AnalyzerInput(buffer: try converter.convert(buffer)))
        }
    } else {
        try await allowMicrophone()
        let engine = AVAudioEngine()
        let mic = engine.inputNode
        mic.installTap(onBus: 0, bufferSize: 4096, format: mic.outputFormat(forBus: 0)) { buffer, _ in
            if let converted = try? converter.convert(buffer) {
                feed.yield(AnalyzerInput(buffer: converted))
            }
        }
        engine.prepare()
        try engine.start()
        emit(["listening": true])
        // Until lemur closes stdin.
        for try await _ in FileHandle.standardInput.bytes {}
        engine.stop()
        mic.removeTap(onBus: 0)
    }
    feed.finish()
    try await analyzer.finalizeAndFinishThroughEndOfInput()
    try await results.value
    emit(["done": true])
}

/// The transcriber for `locale`, the locale it supports that by, and the
/// task that tells `transcript` what it hears. SpeechTranscriber is newer,
/// but knows fewer languages than the dictation model, Norwegian among them.
func transcriber(
    for locale: Locale, into transcript: Transcript
) async throws -> (any SpeechModule, Locale, Task<Void, Error>) {
    if let supported = among(await SpeechTranscriber.supportedLocales, locale) {
        let module = SpeechTranscriber(
            locale: supported, transcriptionOptions: [], reportingOptions: [.volatileResults],
            attributeOptions: [])
        let results = Task {
            for try await result in module.results {
                transcript.heard(String(result.text.characters), final: result.isFinal)
            }
        }
        return (module, supported, results)
    }
    if let supported = among(await DictationTranscriber.supportedLocales, locale) {
        let module = DictationTranscriber(
            locale: supported, contentHints: [], transcriptionOptions: [.punctuation],
            reportingOptions: [.volatileResults], attributeOptions: [])
        let results = Task {
            for try await result in module.results {
                transcript.heard(String(result.text.characters), final: result.isFinal)
            }
        }
        return (module, supported, results)
    }
    throw Failure("this Mac can't transcribe \(locale.identifier)")
}

/// `locale` as one of `locales`. (`supportedLocale(equivalentTo:)` gives
/// back locales the module doesn't support too.)
func among(_ locales: [Locale], _ locale: Locale) -> Locale? {
    locales.first { $0.identifier(.bcp47) == locale.identifier(.bcp47) }
}

/// Asks for the microphone the first time; macOS remembers the answer.
func allowMicrophone() async throws {
    switch AVCaptureDevice.authorizationStatus(for: .audio) {
    case .authorized: return
    case .notDetermined:
        if await AVCaptureDevice.requestAccess(for: .audio) { return }
        // Still undecided: macOS didn't ask, so Settings doesn't list lemur.
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            throw Failure(
                "macOS wouldn't ask for the microphone, as the app that started Lemur can't use it; open Lemur.app itself")
        }
    default: break
    }
    throw Failure(
        "Lemur may not use the microphone; allow it in System Settings → Privacy & Security → Microphone")
}

/// The words heard so far: finalized ones stay, the volatile rest is a
/// guess that later results replace.
final class Transcript {
    var finalized = ""
    var volatile = ""

    func heard(_ text: String, final: Bool) {
        if final {
            finalized += text
            volatile = ""
        } else {
            volatile = text
        }
        emit(["finalized": finalized, "volatile": volatile])
    }
}

/// Converts audio to the format the analyzer takes, one buffer at a time.
final class Converter {
    let format: AVAudioFormat
    var converter: AVAudioConverter?

    init(to format: AVAudioFormat) { self.format = format }

    func convert(_ buffer: AVAudioPCMBuffer) throws -> AVAudioPCMBuffer {
        if buffer.format == format { return buffer }
        if converter?.inputFormat != buffer.format {
            converter = AVAudioConverter(from: buffer.format, to: format)
            // No priming: it would drop the start of each buffer.
            converter?.primeMethod = .none
        }
        guard let converter else { throw Failure("can't convert \(buffer.format) audio") }
        let ratio = format.sampleRate / buffer.format.sampleRate
        let frames = AVAudioFrameCount((Double(buffer.frameLength) * ratio).rounded(.up))
        let converted = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)!
        var given = false
        var error: NSError?
        let status = converter.convert(to: converted, error: &error) { _, status in
            if given {
                status.pointee = .noDataNow
                return nil
            }
            given = true
            status.pointee = .haveData
            return buffer
        }
        if status == .error { throw error ?? Failure("audio conversion failed") }
        return converted
    }
}
