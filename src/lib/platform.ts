/**
 * The OS this window is running on, for the labels that name a key.
 *
 * Read from the webview's user agent rather than from a command: hints have to be
 * right on the first paint, and the agent is already in hand before any IPC round
 * trip. WKWebView reports "Macintosh", WebKitGTK "X11; Linux", WebView2 "Windows NT".
 */
export const isMac = /Mac/i.test(navigator.userAgent);
