import Foundation
import MLX
import MLXLMCommon
import Network

struct Reply {
    var status: Int
    var body: [String: Any]
}

/// The KV cache of the previous prompt. Every cleanup shares the long system prompt, so only the
/// new suffix is prefilled (about 240 ms saved per request with E4B on an M5).
final class PrefixCache: @unchecked Sendable {
    var cache: [KVCache]?
    var tokens: [Int] = []
}

/// Holds the model and runs one generation at a time, in arrival order.
actor Engine {
    private let prefix = PrefixCache()
    private var container: ModelContainer?
    private var last: Task<Void, Never> = Task {}

    func loaded(_ container: ModelContainer) { self.container = container }
    var isReady: Bool { container != nil }

    func complete(_ body: [String: Any]) async -> Reply {
        guard let container else { return Reply(status: 503, body: ["error": "model is loading"]) }
        let previous = last
        let job = Task { await previous.value; return await Engine.generate(container, self.prefix, body) }
        last = Task { _ = await job.value }
        return await job.value
    }

    private static func generate(_ container: ModelContainer, _ prefix: PrefixCache, _ body: [String: Any]) async -> Reply {
        guard let raw = body["messages"] as? [[String: Any]] else {
            return Reply(status: 400, body: ["error": "messages missing"])
        }
        let messages = raw.map {
            Chat.Message(
                role: Chat.Message.Role(rawValue: $0["role"] as? String ?? "") ?? .user,
                content: $0["content"] as? String ?? "")
        }
        let maxTokens = body["max_tokens"] as? Int ?? 4096
        let temperature = (body["temperature"] as? NSNumber)?.floatValue ?? 0
        do {
            let (text, reason) = try await container.perform { context in
                // Thinking stays off whatever the caller sends: cleanup must answer directly.
                let input = try await context.processor.prepare(
                    input: UserInput(chat: messages, additionalContext: ["enable_thinking": false]))
                let parameters = GenerateParameters(maxTokens: maxTokens, temperature: temperature)
                let tokens = input.text.tokens.asArray(Int.self)
                var cache = prefix.cache
                var start = 0
                if let old = cache {
                    // Keep the shared prefix (at least one token must still be fed) and rewind the rest.
                    let keep = min(zip(prefix.tokens, tokens).prefix { $0 == $1 }.count, tokens.count - 1)
                    let drop = (old.first?.offset ?? 0) - keep
                    if keep > 0, canTrimPromptCache(old), trimPromptCache(old, numTokens: drop) == drop {
                        start = keep
                    } else {
                        cache = nil
                    }
                }
                if cache == nil { cache = try context.model.newCache(parameters: parameters) }
                prefix.cache = nil
                let feed = start == 0 ? input : LMInput(tokens: MLXArray(tokens[start...].map { Int32($0) }))
                var text = ""
                var reason = "stop"
                for await event in try MLXLMCommon.generate(
                    input: feed, cache: cache, parameters: parameters, context: context) {
                    switch event {
                    case .chunk(let s): text += s
                    case .info(let info): if case .length = info.stopReason { reason = "length" }
                    default: break
                    }
                }
                (prefix.cache, prefix.tokens) = (cache, tokens)
                return (text, reason)
            }
            let message: [String: Any] = ["role": "assistant", "content": text]
            return Reply(status: 200, body: ["choices": [["finish_reason": reason, "message": message]]])
        } catch {
            return Reply(status: 500, body: ["error": "\(error)"])
        }
    }
}

/// Minimal HTTP/1.1 on 127.0.0.1: one request per connection, then close.
struct Server {
    let port: UInt16
    let token: String
    let engine: Engine
    private static let maxRequest = 8 << 20

    func start() throws {
        let params = NWParameters.tcp
        params.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: NWEndpoint.Port(rawValue: port)!)
        let listener = try NWListener(using: params)
        listener.newConnectionHandler = { conn in
            conn.start(queue: .global())
            self.read(conn, Data())
        }
        listener.stateUpdateHandler = { state in
            if case .failed(let error) = state { fail("listener failed: \(error)", code: 1) }
        }
        listener.start(queue: .global())
    }

    private func read(_ conn: NWConnection, _ buffer: Data) {
        conn.receive(minimumIncompleteLength: 1, maximumLength: 1 << 16) { data, _, done, error in
            var buffer = buffer
            if let data { buffer.append(data) }
            if let end = buffer.range(of: Data("\r\n\r\n".utf8)) {
                let head = String(decoding: buffer[..<end.lowerBound], as: UTF8.self)
                let lines = head.components(separatedBy: "\r\n")
                var headers: [String: String] = [:]
                for line in lines.dropFirst() {
                    if let colon = line.firstIndex(of: ":") {
                        headers[line[..<colon].lowercased()] =
                            line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
                    }
                }
                let length = Int(headers["content-length"] ?? "0") ?? 0
                let body = buffer[end.upperBound...]
                if length > Self.maxRequest { return self.send(conn, Reply(status: 413, body: [:])) }
                if body.count >= length {
                    let parts = lines[0].split(separator: " ")
                    let method = parts.count > 1 ? String(parts[0]) : ""
                    let path = parts.count > 1 ? String(parts[1]) : ""
                    Task {
                        let reply = await self.handle(method, path, headers["authorization"], Data(body.prefix(length)))
                        self.send(conn, reply)
                    }
                    return
                }
            }
            if done || error != nil || buffer.count > Self.maxRequest { return conn.cancel() }
            self.read(conn, buffer)
        }
    }

    private func handle(_ method: String, _ path: String, _ auth: String?, _ body: Data) async -> Reply {
        switch (method, path) {
        case ("GET", "/health"):
            return await engine.isReady
                ? Reply(status: 200, body: ["status": "ok"]) : Reply(status: 503, body: ["status": "loading"])
        case ("POST", "/v1/chat/completions"):
            guard let auth, equalInConstantTime(auth, "Bearer \(token)") else {
                return Reply(status: 401, body: ["error": "unauthorized"])
            }
            guard let json = try? JSONSerialization.jsonObject(with: body) as? [String: Any] else {
                return Reply(status: 400, body: ["error": "invalid JSON"])
            }
            return await engine.complete(json)
        default:
            return Reply(status: 404, body: ["error": "not found"])
        }
    }

    private func send(_ conn: NWConnection, _ reply: Reply) {
        let body = (try? JSONSerialization.data(withJSONObject: reply.body)) ?? Data("{}".utf8)
        let head = "HTTP/1.1 \(reply.status) \(HTTPURLResponse.localizedString(forStatusCode: reply.status))\r\n"
            + "Content-Type: application/json\r\nContent-Length: \(body.count)\r\nConnection: close\r\n\r\n"
        conn.send(content: Data(head.utf8) + body, completion: .contentProcessed { _ in conn.cancel() })
    }
}

private func equalInConstantTime(_ a: String, _ b: String) -> Bool {
    let (x, y) = (Array(a.utf8), Array(b.utf8))
    guard x.count == y.count else { return false }
    return zip(x, y).reduce(0) { $0 | ($1.0 ^ $1.1) } == 0
}
