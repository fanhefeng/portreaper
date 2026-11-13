# macOS window and quit regression checklist

Apple defines [Cmd+Q as Quit, Cmd+W as Close Window, and Cmd+M as Minimize](https://support.apple.com/en-us/102650).
Portreaper keeps its menu bar utility identity: closing the main window hides it
and leaves the tray running. An explicit Quit fully exits, including the tray.

Run these checks against a native macOS build (`pnpm tauri dev` or the bundled
app). Webview unit tests cannot verify AppKit key equivalents, process exit or
the system tray.

- [ ] With the main window focused, Cmd+W hides it. The process and tray icon
      remain. Clicking the tray icon or choosing **Show Window** restores focus.
- [ ] The red close button behaves like Cmd+W.
- [ ] Cmd+M minimizes the window. Clicking the tray icon or choosing **Show
      Window** restores it, including keyboard focus.
- [ ] After hiding the window, relaunching the same app restores it via Reopen.
- [ ] With the main window focused, Cmd+Q exits the process and removes the tray
      icon. It must not merely hide the window.
- [ ] The tray **Quit Portreaper** command also exits, including while the main
      window is hidden or minimized.
- [ ] Resize/move the window, Cmd+Q, then relaunch. Window geometry is restored
      and the main window is visible. Quit uses `app.exit(0)` so the window-state
      plugin receives `RunEvent::Exit` before cleanup.
- [ ] Switch between Chinese and English; both Quit items use the same localized
      **退出 Portreaper** / **Quit Portreaper** label, never **Hide to Tray**.
- [ ] Cmd+C/V/X/A still work in the search field, and Cmd+, opens Settings.
- [ ] No Dock icon or Cmd+Tab entry appears, including at cold launch.

Logout/shutdown must also be allowed to quit. The existing native AppleEvent
quit path bypasses Tauri's `RunEvent::Exit` on this stack, so geometry changes
since the last Cmd+Q or tray Quit can be lost on that path.

Native verification on 2026-11-04 (debug app bundle): before the fix, Cmd+Q hid
the window but left the process alive. After the fix, Cmd+W left the process
running and reopening restored the window; Cmd+Q exited the process and wrote
the window-state file. Cmd+, opened Settings, and Cmd+Q also exited while that
dialog was open. These checks do not cover logout/shutdown or a release bundle.
