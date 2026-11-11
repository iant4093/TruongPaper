# TruongPaper desktop acceptance testing

Use this checklist after the automated checks pass. Test the actual desktop app, not `npm run dev`: the browser preview uses sample data and cannot change your wallpaper.

Start with the macOS checks (about 20–30 minutes), then Steam and one wallpaper of each type (another 15–30 minutes). Multi-display and Windows checks can be done separately when those devices are available. Stop at the first failure in a section and send the report format at the end; you do not need to debug it yourself.

## Start here: the new Workshop browser and RAM meter

The local Mac build is **1.0.4** and is already running. These are the first checks that need your account or your observation:

1. Open Workshop → Discover without signing in. Search a subject, change Type/Genre/Resolution, and try Next/Previous. Open a preview and check the download choices.
2. Check that the sidebar has separate RAM and Disk cache rows and no status dot. RAM refreshes about every three seconds and reports the main process; helper processes and GPU memory are separate.
3. Paste `3836199874` into Import and choose **Download via GGNetwork**. When it appears in Installed, apply it and watch a few loops. GGNetwork availability can change. Test **Download with Steam** separately using an account with Wallpaper Engine access.
4. Paste another direct Workshop URL or ID into the top Import field. Also check an existing photo-album link.
5. Watch the wallpaper for three loops, try Pause/Resume, then lock/unlock and sleep/wake. Repeat the guest browsing and download/apply checks on Windows when available.

The native guest gallery and GGNetwork download of `3836199874` passed. The packaged Steam downloader started and displayed a live QR. The 159 frontend tests and 405 Rust tests passed. Playback, downloading through a Steam account with Wallpaper Engine access, and Windows desktop behavior still need desktop testing. Detailed checks follow below.

## Priority retest: fill, gray flashes, and branding

The current local Mac test build is **1.0.4**, including the wallpaper fixes, new icon, RAM reading, and Workshop browser. Confirm that version in Settings. Windows CI artifacts use their Actions run version; select the latest successful run for the current app changes.

1. In Displays, choose **Fill / Center Crop**. It should affect the photo already on the desktop immediately. New installations now default to Fill; existing explicit fit choices are preserved.
2. Minimize Studio and watch the desktop for at least 60 seconds. The picture should reach every edge without blurred strips, and should not alternate with a blank gray screen.
3. Bring Studio forward and hide it several times, rotate through three photos, and test Pause/Resume. Then lock/unlock once. A static photo should redraw after resume without repeated window destruction.
4. Check the new layered-photo icon in the Dock, Finder, app sidebar, and browser tab. The menu bar should use a small transparent photo mark rather than a solid black tile.
5. Check for gray flashing after sleep or repeated rotations.
6. If you prefer a defined menu bar edge, enable System Settings → Menu Bar → Show menu bar background. This removed the wide fade on the test Mac.

## 1. Open the current build

1. Quit any already-running TruongPaper from its menu before opening a newer copy.
2. On this Mac, open `src-tauri/target/release/bundle/macos/TruongPaper.app`, or install the `.dmg` from `src-tauri/target/release/bundle/dmg/` into Applications.
3. On Windows, download `installers-Windows` from the latest successful **CI** run on `main`, unzip it, and run `TruongPaper-Windows-Setup.exe`. These are Actions artifacts, not a published release.
4. If macOS blocks this ad-hoc signed app, handle the normal **System Settings → Privacy & Security → Open Anyway** flow yourself. Do not change global security settings.
5. For development logs instead, run this from the repository and leave the terminal open:

   ```bash
   npm run app:dev > /tmp/truongpaper-manual-test.log 2>&1
   ```

**Pass:** The simple dark Library opens. Sidebar navigation, the display selector, import field, and Pause/Next controls are visible. There are no glass/refraction effects or colored UI glows. The lower playback bar shows your actual current wallpaper rather than an unused queue photo.

## 2. Check the new interface

1. Open Library, Photography, Workshop, Playlists, Displays, and Settings.
2. Resize the window down to its minimum size, then maximize it. Scroll each long page.
3. After importing a Workshop wallpaper in step 5, click its thumbnail. Its details and property controls should open. Toggle details off, select another wallpaper, and confirm details update.
4. In Library, search for a title and try the Scene, Video, and Web filters. Clear the search.
5. Press Tab through the visible controls. Focus should remain visible and fields should accept normal text selection/copy/paste.
6. Close the Studio window. The wallpaper should keep running. On macOS, click the TruongPaper Dock icon to reopen Studio. Also check Open TruongPaper Studio from the menu bar or Windows system tray.

**Pass:** Text is readable, controls fit, scrolling reaches every setting, and selecting a thumbnail does not apply a wallpaper until Apply is used. Closing Studio hides the window without quitting the engine.

## 3. Photos and offline rotation

1. In Library, Apply **Ian Truong Photography** if needed. Wait for the Photography queue to fill.
2. Click Next three times. Each successful change should update the current-photo information and desktop wallpaper.
3. Hide Studio and look at the desktop: the photo must appear behind the desktop icons, without blocking clicks or dragging icons.
4. In Photography, switch between all public photos and featured photos. Browse albums and save one you recognize. Enable the saved-albums source and confirm new photos belong to it.
5. Favorite the current photo, then hide a different photo. Verify the hidden photo is skipped and the favorite remains marked after reopening Studio.
6. When at least three unused photos are queued, temporarily disconnect Wi-Fi. Click Next twice, then reconnect Wi-Fi and refill the queue.
7. In Playlists, set rotation to one minute. Verify one timed change. Select Never, wait at least 70 seconds, and verify no automatic change. Restore your preferred interval.
8. Try Crossfade, Blur-Dissolve, and Liquid Glass Morph at short and long durations. These are wallpaper transitions; the app interface remains flat and dark.

**Pass:** Queued photos work offline, history grows after successful photo changes, hidden items do not return, and a failed network refill leaves the current wallpaper visible.

## 4. Pause, desktop behavior, and power

Use an animated wallpaper after step 5 for the strongest pause check; a still photo cannot demonstrate that animation stopped.

1. Pause and Resume from Studio and from the tray. Check animation and sound, not only the text label.
2. Enable fullscreen Smart Pause, open another app fullscreen, then leave fullscreen. Animation/audio should pause and resume on the affected display.
3. Enable maximized-window pause and repeat with a maximized window. Turn that option back to your preference afterward.
4. Lock/unlock the Mac or PC, then sleep/wake it. The wallpaper should return without an extra wallpaper window, black screen, stuck audio, or crash.
5. On a laptop, unplug power and test your selected battery policy; also try Low Power Mode where available. Restore your normal settings.
6. On macOS, switch Spaces and use Mission Control. Desktop icons must remain usable and the wallpaper must stay at desktop level.
7. Watch Activity Monitor/Task Manager while an animated wallpaper plays, then pauses. Rendering activity should decrease substantially. Do not require a literal zero CPU reading.

**Pass:** User pause, automatic pause, audio, and resume agree. Record any mismatch between the label and actual behavior.

## 5a. Public Workshop browsing — no login needed

1. Open Workshop → Discover before signing in. Real thumbnails, creator names, ratings, and subscriber counts should appear.
2. Search `mountain`. Try Trending / Most subscribed / Newest / Top rated, change the period, then filter Type to Scene, choose a Genre or Resolution, and try the extra Filters. Clear everything afterward.
3. Use Next, Previous, and the page number field. The page label and wallpaper results should change. Steam caps public browsing at 1,000 pages; narrow a search to reach more specific results.
4. Click a wallpaper. Its preview should open without downloading. Close it, then try pasting a Workshop link or numeric ID into Discover's search field and press Search.
5. Disconnect Wi-Fi and change the search; expect an error and Retry. Reconnect and retry. Installed wallpapers should remain available under Installed.
6. Check the sidebar's separate Disk cache and RAM rows. RAM updates about every three seconds and means the main app process's resident memory; WebKit helpers and GPU allocations are separate. It should not equal the photo-cache size or show a colored status dot.

**Pass:** Browsing is available without sign-in. Previewing never auto-downloads. The top Import field still accepts direct Workshop links/IDs and photo album links and opens a preview for Workshop items. Discover always uses Steam’s public guest catalog.

## 5. Steam sign-in and real downloads — your account is needed

1. Go to Workshop → Steam account (or Installed → Manage Steam Account). Complete the Steam Mobile QR flow yourself. Use an account with access to Wallpaper Engine for the download test. Do not paste passwords, QR screenshots, Steam Guard codes, or tokens into this chat.
2. In Discover, preview a small Wallpaper Engine Workshop item you have access to, then choose Download with Steam. Also test a direct link/ID using the top Import field. If core assets need to download, let that finish too.
3. Confirm download progress reaches completion, a thumbnail appears, and Apply works.
4. Test a password/Steam Guard login separately if you use that method. Sign out first; enter credentials only in the app.
5. If Steam is installed locally, use Sync Local Steam Library. Confirm a subscribed wallpaper appears with its preview and can be applied.
6. Quit and reopen the packaged app. Confirm the bundled downloader is found and the account does not repeatedly request Keychain access for every operation.

**Pass:** The downloader completes authentication/downloads and works from the packaged app. A login failure should show an understandable error without losing the current wallpaper.

**Known:** Signing out clears TruongPaper's vault and stops its sidecar; DepotDownloader may retain its own saved login separately.

## 6. Check each wallpaper runtime

Use actual Workshop IDs/links in your report so failures can be reproduced.

1. **Video:** Play a short video for at least three loops. Try mute/unmute, volume, playback speed, and pause/resume. There should be no black frame or stopped playback at loop boundaries.
2. **Web:** Pick an interactive wallpaper. Change a property, move the cursor over the desktop, and check pause/resume. The wallpaper should not steal desktop clicks or keyboard focus.
3. **Scene:** Start with a simple image/effect scene, then try one with particles or a puppet. Check animation, colors, property controls, and cursor/parallax behavior.
4. **Embedded scene video, Mac:** Try a scene whose `.tex` contains video. The separate decoder probe passed, but real texture updates and looping still need this check.
5. **Audio reactive:** Play system music, then stop it. The spectrum should react and decay to silence. Switch to a non-reactive wallpaper and check that capture stops. **On macOS, the current implementation requires a configured virtual loopback device such as BlackHole; native system-audio capture is not integrated. Skip this check if you do not already have one.**
6. **Windows application:** On Windows only, use a trusted application wallpaper. Apply it, then switch back to a photo. Its process/window should start and stop with the wallpaper.

**Known limits:** Embedded scene MP4 textures are not decoded on Windows yet. Morph targets and sound-layer repeat scheduling are incomplete. Some complex scenes may require additional compatibility fixes. These are not pass criteria for this preview.

## 7. Multiple displays, if available

1. Select each display and apply a different wallpaper to it. Confirm the correct physical display changes.
2. Put a web wallpaper on one display and a video wallpaper on the other. Each must use its own files; neither should show the other's assets.
3. Apply one scene to both displays. Its sound should play once rather than doubling. Note any restart when the display owning sound is auto-paused; this remains a known limitation.
4. Test portrait versus landscape matching and each Fit option.
5. Create a playlist with at least three items and a one-minute interval. Once it has advanced, unplug and reconnect the external display. It should restore its current item rather than restarting the playlist.
6. Change resolution/scaling and move a display in OS display settings. Windows should resize/reposition correctly; disconnects should not move wallpaper onto the wrong monitor.

**Known:** A single canvas spanning displays is not implemented. Applying the same source to all displays does not synchronize independent photo draws.

## 8. Windows desktop coverage and updates

1. Repeat the basic photo/video/web/scene and tray tests on Windows.
2. Record the exact Windows version/build from `winver`. Windows 11 24H2 or later and the older desktop attachment path both need real-device coverage.
3. If comfortable doing so, restart Windows Explorer from Task Manager and check that the wallpaper reattaches. Otherwise skip this disruption and say so.
4. Open Settings → Check for Updates. Before the first release exists, an unavailable-feed error is expected; the app must remain usable.
5. After a first release and a newer signed release are available, test accepting an update: download, installation, restart, and version change. This two-version test cannot be completed before releases exist.

## Send results back

Copy this short report for each failure:

```text
Platform and OS build:
TruongPaper build/commit:
Display count, resolutions, scaling:
Checklist step:
Wallpaper Workshop ID/link (if applicable):
What I clicked:
Expected:
Actual:
Reproduces: every time / sometimes / once
Screenshot or short recording:
Relevant error/log lines (remove credentials and tokens):
```

A useful first reply is simply: **Steps 1–3 passed**, followed by the first failing step. Device-dependent sections can be marked **skipped — no Windows machine / external display / virtual audio device**.
