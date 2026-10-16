// cache-siren overlay — always-on-top, non-activating floating panel hosting the daemon's page.
// No Accessibility / AppleEvents needed (no TCC popups). Position/size persist via frame autosave.
// Follows GET /ctl {visible, opacity} every 2s so `cache-siren show|hide` from any shell works.
import Cocoa
import WebKit

let PORT = Int(ProcessInfo.processInfo.environment["CACHE_SIREN_PORT"] ?? "9022") ?? 9022
let BASE = "http://127.0.0.1:\(PORT)"
let logURL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".claude/cache-siren/overlay.log")

func log(_ msg: String) {
    let ts = ISO8601DateFormatter().string(from: Date())
    let line = "[\(ts)] [cache-siren-overlay] \(msg)\n"
    if let h = try? FileHandle(forWritingTo: logURL) {
        let end = h.seekToEndOfFile()
        if end > 5_000_000 { try? h.truncate(atOffset: 0) }
        h.write(line.data(using: .utf8)!); try? h.close()
    } else { try? line.data(using: .utf8)!.write(to: logURL) }
}

final class DragWebView: WKWebView {
    // Borderless panel + WKWebView: the web view swallows mouse-downs, so the panel never drags.
    // Returning true lets a press-and-drag anywhere move the window; plain clicks still reach page buttons.
    override var mouseDownCanMoveWindow: Bool { true }
}

final class Panel: NSPanel {
    override var canBecomeKey: Bool { true }      // so buttons in the page are clickable
    override var canBecomeMain: Bool { false }
}

final class App: NSObject, NSApplicationDelegate, WKNavigationDelegate, WKScriptMessageHandler {
    var panel: Panel!
    var web: WKWebView!
    var visible = true
    var page = "/"
    var retry: Timer?

    func applicationDidFinishLaunching(_ n: Notification) {
        let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let w: CGFloat = 640, h: CGFloat = 320
        let frame = NSRect(x: screen.maxX - w - 12, y: screen.maxY - h - 12, width: w, height: h)
        panel = Panel(contentRect: frame, styleMask: [.borderless, .nonactivatingPanel, .resizable], backing: .buffered, defer: false)
        panel.isFloatingPanel = true
        panel.level = .statusBar                      // same layer ultralog uses — above normal windows, below menus
        panel.hidesOnDeactivate = false
        panel.isMovableByWindowBackground = true
        panel.collectionBehavior = [.canJoinAllSpaces, .stationary, .fullScreenAuxiliary, .ignoresCycle]
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.alphaValue = 0.94
        panel.hasShadow = true
        panel.minSize = NSSize(width: 360, height: 120)
        panel.setFrameAutosaveName("CacheSirenPanel")

        let cfg = WKWebViewConfiguration()
        cfg.userContentController.add(self, name: "ctl")
        web = DragWebView(frame: panel.contentView!.bounds, configuration: cfg)
        web.autoresizingMask = [.width, .height]
        web.navigationDelegate = self
        web.setValue(false, forKey: "drawsBackground")
        web.wantsLayer = true
        web.layer?.cornerRadius = 8
        web.layer?.masksToBounds = true
        panel.contentView?.addSubview(web)
        load()
        panel.orderFrontRegardless()
        log("launched port=\(PORT) frame=\(panel.frame)")
        Timer.scheduledTimer(withTimeInterval: 2.0, repeats: true) { [weak self] _ in self?.pollCtl() }
    }

    func load() { web.load(URLRequest(url: URL(string: BASE + page)!, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 5)) }

    func webView(_ w: WKWebView, didFail n: WKNavigation!, withError e: Error) { scheduleRetry("didFail \(e.localizedDescription)") }
    func webView(_ w: WKWebView, didFailProvisionalNavigation n: WKNavigation!, withError e: Error) { scheduleRetry("provisional \(e.localizedDescription)") }
    func webView(_ w: WKWebView, didFinish n: WKNavigation!) { if retry != nil { log("loaded after retry") } else { log("loaded") }; retry?.invalidate(); retry = nil }
    func scheduleRetry(_ why: String) {
        if retry != nil { return }
        log("load failed (\(why)) — retry in 3s")
        retry = Timer.scheduledTimer(withTimeInterval: 3.0, repeats: true) { [weak self] _ in self?.load() }
    }

    func userContentController(_ c: WKUserContentController, didReceive m: WKScriptMessage) {
        if let s = m.body as? String, s == "hide" { setVisible(false) }
    }

    func setVisible(_ v: Bool) {
        if v == visible { return }
        visible = v
        if v { panel.orderFrontRegardless(); load() } else { panel.orderOut(nil) }
        log("visible -> \(v)")
    }

    func pollCtl() {
        var req = URLRequest(url: URL(string: BASE + "/ctl")!, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: 2)
        req.httpMethod = "GET"
        URLSession.shared.dataTask(with: req) { [weak self] data, _, _ in
            guard let self = self, let d = data, let j = try? JSONSerialization.jsonObject(with: d) as? [String: Any] else { return }
            DispatchQueue.main.async {
                if let v = j["visible"] as? Bool { self.setVisible(v) }
                if let o = j["opacity"] as? Double, abs(self.panel.alphaValue - o) > 0.01 { self.panel.alphaValue = o }
                if let p = j["page"] as? String, p != self.page {   // switch between live ('/') and demo ('/demo')
                    self.page = p
                    log("page -> \(p)")
                    if self.visible { self.load() }
                }
            }
        }.resume()
    }
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = App()
app.delegate = delegate
app.run()
