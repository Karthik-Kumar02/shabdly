// shabdly-engine: serves one MLX model over local HTTP for Shabdly.
// Usage: SHABDLY_ENGINE_TOKEN=<hex> shabdly-engine --model <dir> --port <n> --parent-pid <pid>
import Foundation
import MLXLLM
import MLXLMCommon
import Tokenizers

func fail(_ message: String, code: Int32) -> Never {
    FileHandle.standardError.write(Data("shabdly-engine: \(message)\n".utf8))
    exit(code)
}

var args: [String: String] = [:]
var rest = CommandLine.arguments.dropFirst()
while let key = rest.popFirst() {
    guard key.hasPrefix("--"), let value = rest.popFirst() else { fail("bad arguments", code: 1) }
    args[String(key.dropFirst(2))] = value
}
guard let modelDir = args["model"], let port = UInt16(args["port"] ?? ""),
    let parentPid = Int32(args["parent-pid"] ?? "")
else { fail("usage: --model <dir> --port <n> --parent-pid <pid>", code: 1) }
// The token comes only from the environment: arguments are visible to other users in `ps`.
guard let token = ProcessInfo.processInfo.environment["SHABDLY_ENGINE_TOKEN"], !token.isEmpty else {
    fail("SHABDLY_ENGINE_TOKEN is not set", code: 1)
}

// Orphan protection: exit when the parent (Shabdly) is gone, so a force-quit does not leave 4 GB behind.
let parentWatch = DispatchSource.makeTimerSource(queue: .global())
parentWatch.schedule(deadline: .now() + 1, repeating: 1)
parentWatch.setEventHandler { if kill(parentPid, 0) != 0 { exit(0) } }
parentWatch.resume()

let engine = Engine()
do {
    try Server(port: port, token: token, engine: engine).start()
} catch {
    fail("cannot listen on 127.0.0.1:\(port): \(error)", code: 1)
}

/// mlx-swift-lm 3.x leaves tokenizers to the caller; this bridges swift-transformers' AutoTokenizer.
struct TransformersLoader: MLXLMCommon.TokenizerLoader {
    func load(from directory: URL) async throws -> any MLXLMCommon.Tokenizer {
        Bridge(upstream: try await AutoTokenizer.from(modelFolder: directory))
    }

    struct Bridge: MLXLMCommon.Tokenizer {
        let upstream: any Tokenizers.Tokenizer
        func encode(text: String, addSpecialTokens: Bool) -> [Int] {
            upstream.encode(text: text, addSpecialTokens: addSpecialTokens)
        }
        func decode(tokenIds: [Int], skipSpecialTokens: Bool) -> String {
            upstream.decode(tokens: tokenIds, skipSpecialTokens: skipSpecialTokens)
        }
        func convertTokenToId(_ token: String) -> Int? { upstream.convertTokenToId(token) }
        func convertIdToToken(_ id: Int) -> String? { upstream.convertIdToToken(id) }
        var bosToken: String? { upstream.bosToken }
        var eosToken: String? { upstream.eosToken }
        var unknownToken: String? { upstream.unknownToken }
        func applyChatTemplate(
            messages: [[String: any Sendable]], tools: [[String: any Sendable]]?,
            additionalContext: [String: any Sendable]?
        ) throws -> [Int] {
            do {
                return try upstream.applyChatTemplate(
                    messages: messages, tools: tools, additionalContext: additionalContext)
            } catch Tokenizers.TokenizerError.missingChatTemplate {
                throw MLXLMCommon.TokenizerError.missingChatTemplate
            }
        }
    }
}

Task {
    do {
        let container = try await LLMModelFactory.shared.loadContainer(
            from: URL(fileURLWithPath: modelDir), using: TransformersLoader())
        await engine.loaded(container)
    } catch {
        fail("model failed to load: \(error)", code: 2)
    }
}

dispatchMain()
