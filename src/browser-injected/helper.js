// Built-in browser helper. Injected by the Rust host into the ISOLATED world
// of every browser tab (WKContentWorld "codeg" on macOS, a CDP isolated world
// on Windows, a WebKitGTK script world on Linux) at document start, for every
// frame. The page cannot see this script, its globals, or the native message
// channel it talks over — an isolated world shares the DOM but has its own
// JavaScript globals, so `JSON`, `addEventListener` and friends here are
// pristine even when the page overrides its own copies.
//
// It does exactly two things:
//   1. Reports navigation state the host cannot observe natively (SPA
//      `pushState` / `replaceState` / hash changes, document title changes)
//      as `nav-state` messages.
//   2. Records user gestures — click / auxclick / keydown in the capture
//      phase — as `gesture` messages, including the anchor under the
//      pointer, so the host can decide how to present a resulting new-window
//      request (adopt as a tab vs. popup) and can turn a modifier-click on a
//      plain anchor into a background tab from `on_navigation`. It never
//      cancels, rewrites or re-dispatches anything: the engine navigates, the
//      host only decides presentation.
//
// The host defines `__codegSend(string)` before this script runs. Messages
// are `{ kind, payload }` JSON strings; the host treats every field as
// untrusted input.
;(function codegBrowserHelper() {
  "use strict"
  if (typeof globalThis.__codegSend !== "function") return
  if (globalThis.__codegHelperInstalled) return
  globalThis.__codegHelperInstalled = true

  var send = globalThis.__codegSend
  var stringify = JSON.stringify
  var now = function () {
    return Date.now()
  }
  var isTop = (function () {
    try {
      return window.top === window
    } catch {
      return false
    }
  })()

  function post(kind, payload) {
    try {
      send(stringify({ kind: kind, payload: payload, top: isTop }))
    } catch {
      /* the channel is best-effort; never throw into page event dispatch */
    }
  }

  // ---- navigation state -------------------------------------------------
  var lastHref = ""
  var lastTitle = ""
  function reportNav(reason) {
    var href = String(location.href)
    var title = String(document.title || "")
    if (href === lastHref && title === lastTitle) return
    lastHref = href
    lastTitle = title
    post("nav-state", { href: href, title: title, reason: reason })
  }
  // history.pushState / replaceState are the only SPA transitions with no
  // native signal at all; wrapping them in this world affects only calls made
  // from this world, so we observe the page's calls through the events they
  // produce instead: none. Poll cheaply on user-visible ticks plus the events
  // that do fire.
  window.addEventListener(
    "popstate",
    function () {
      reportNav("popstate")
    },
    true
  )
  window.addEventListener(
    "hashchange",
    function () {
      reportNav("hashchange")
    },
    true
  )
  document.addEventListener(
    "DOMContentLoaded",
    function () {
      reportNav("dom-ready")
    },
    true
  )
  window.addEventListener(
    "load",
    function () {
      reportNav("load")
    },
    true
  )
  if (isTop) {
    var titleObserver = null
    function observeTitle() {
      if (titleObserver || !document.documentElement) return
      try {
        titleObserver = new MutationObserver(function () {
          reportNav("mutation")
        })
        // <title> lives in <head>; observing the document element catches it
        // being created, replaced or edited without walking the whole body.
        titleObserver.observe(document.documentElement, {
          childList: true,
          subtree: true,
          characterData: true,
        })
      } catch {
        titleObserver = null
      }
    }
    observeTitle()
    document.addEventListener("DOMContentLoaded", observeTitle, true)
    // pushState has no event: a light poll on animation frames only while
    // the document is visible costs nothing measurable and catches SPA
    // route changes within a frame.
    var rafHref = ""
    function tick() {
      if (document.visibilityState === "visible") {
        var href = String(location.href)
        if (href !== rafHref) {
          rafHref = href
          reportNav("poll")
        }
      }
      setTimeout(function () {
        requestAnimationFrame(tick)
      }, 250)
    }
    requestAnimationFrame(tick)
  }

  // ---- gestures ----------------------------------------------------------
  var gestureSeq = 0
  function primaryModifier(event) {
    // ⌘ on macOS, Ctrl elsewhere; the host knows the platform, so send both.
    return {
      meta: !!event.metaKey,
      ctrl: !!event.ctrlKey,
      shift: !!event.shiftKey,
      alt: !!event.altKey,
    }
  }
  function anchorFrom(event) {
    var path
    try {
      path =
        typeof event.composedPath === "function" ? event.composedPath() : []
    } catch {
      path = []
    }
    for (var i = 0; i < path.length; i++) {
      var node = path[i]
      if (!node || node.nodeType !== 1) continue
      var tag = String(node.tagName || "").toLowerCase()
      if (tag !== "a" && tag !== "area") continue
      if (!node.hasAttribute || !node.hasAttribute("href")) continue
      var href
      try {
        href = String(node.href || "")
      } catch {
        href = ""
      }
      var rel = String(node.getAttribute("rel") || "").toLowerCase()
      return {
        href: href,
        target: String(node.getAttribute("target") || ""),
        download: node.hasAttribute("download"),
        relNoOpener: /(^|\s)noopener(\s|$)/.test(rel),
        relNoReferrer: /(^|\s)noreferrer(\s|$)/.test(rel),
      }
    }
    return null
  }
  // Synthetic events are ignored: a page can dispatch a fake click, but the
  // host must only ever treat real input as a gesture. The isolated-world
  // flag below is the one exception, set by the host's own test harness
  // through a world-scoped eval (page scripts cannot reach this global).
  function trusted(event) {
    return !!event.isTrusted || globalThis.__codegAcceptUntrusted === true
  }
  function recordGesture(type, event, extra) {
    gestureSeq += 1
    var payload = {
      id: gestureSeq,
      type: type,
      ts: now(),
      button: typeof event.button === "number" ? event.button : -1,
      modifiers: primaryModifier(event),
      anchor: anchorFrom(event),
      isTrusted: !!event.isTrusted,
    }
    if (extra) {
      for (var k in extra) payload[k] = extra[k]
    }
    post("gesture", payload)
    return payload
  }
  window.addEventListener(
    "click",
    function (event) {
      if (!trusted(event)) return
      recordGesture("click", event)
    },
    true
  )
  window.addEventListener(
    "keydown",
    function (event) {
      if (!trusted(event)) return
      if (event.key !== "Enter" && event.key !== " ") return
      recordGesture("keydown", event, { key: event.key })
    },
    true
  )
  window.addEventListener(
    "auxclick",
    function (event) {
      if (!trusted(event)) return
      // Recorded only. Both engines already treat a middle-button auxclick
      // on an anchor as "open in a new tab" (WebKit's HTMLAnchorElement counts
      // it as a link click, WebView2 raises NewWindowRequested), so the
      // request reaches the host's new-window handler on its own; opening it
      // from here as well produced two tabs per middle-click.
      recordGesture("auxclick", event)
    },
    true
  )

  post("hello", {
    href: String(location.href),
    readyState: String(document.readyState),
  })
  reportNav("start")
})()
