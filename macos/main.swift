// cache-siren overlay — always-on-top, non-activating floating panel hosting the daemon's page.
// No Accessibility / AppleEvents needed (no TCC popups). Position/size persist via frame autosave.
// The page carries the API token in its address's fragment, read from CACHE_SIREN_TOKEN or the daemon's
// token file. The page's hide button hides the panel; `bmo-cache-siren restart` brings it back.
import Cocoa
import WebKit

let ENV = ProcessInfo.processInfo.environment
let BASE = ENV["CACHE_SIREN_ADDRESS"] ?? "http://127.0.0.1:9022"
let DATA = ENV["CACHE_SIREN_DATA_DIR"].map { URL(fileURLWithPath: $0) }
    ?? FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent(".claude/cache-siren")
let logURL = DATA.appendingPathComponent("overlay.log")

/// The API token: CACHE_SIREN_TOKEN, else the token file's content; read at every load, since the daemon
/// writes the file on its first start.
func token() -> String {
    if let given = ENV["CACHE_SIREN_TOKEN"], !given.isEmpty { return given }
    let file = ENV["CACHE_SIREN_TOKEN_FILE"].map { URL(fileURLWithPath: $0) } ?? DATA.appendingPathComponent("token")
    let text = (try? String(contentsOf: file, encoding: .utf8)) ?? ""
    return text.trimmingCharacters(in: .whitespacesAndNewlines)
}

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
        cfg.userContentController.add(self, name: "overlay")
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
        log("launched address=\(BASE) frame=\(panel.frame)")
    }

    func load() {
        let address = BASE + "/#access_token=" + token()
        // The load waits for the daemon's answer, ending only when the connection is refused or breaks; the
        // request's idle interval is set past any span the overlay lives through.
        web.load(URLRequest(url: URL(string: address)!, cachePolicy: .reloadIgnoringLocalCacheData, timeoutInterval: TimeInterval(Int32.max)))
    }

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
}

let app = NSApplication.shared
app.setActivationPolicy(.accessory)
let delegate = App()
app.delegate = delegate
app.run()
