# NN0033: Shake to enlarge the cursor

`cursor { shake-to-enlarge }` enables a KDE-style pointer locator. It is off by default. Rapid back-and-forth movement with at least three direction reversals and 180 logical pixels of travel in a 250 ms trail enlarges the cursor to 2.5x, then eases it back after 180 ms without motion. The fixed 16-sample trail avoids per-event allocations. The detector follows KWin's ShakeDetector approach of comparing trail distance with the bounding-box diagonal, with additional reversal and travel gates. Reference: [KWin ShakeDetector source](https://invent.kde.org/plasma/kwin/-/blob/master/src/plugins/shakecursor/shakedetector.cpp).

The native compositor capability is reported as `cursor_shake`; the Nacre helper exposes it to Settings as `cursorShake`. Stock niri and Hyprland report false.
