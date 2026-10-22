# Progress: Phase 5.1 (Hotfix: black screen), v0.5.1-hotfix

**Status:** complete · released as [v0.5.1-hotfix](https://github.com/goglesquirmintontheiii/us-explorer/releases/tag/v0.5.1-hotfix) on 2026-10-22 · previous phase: [Phase 5](progress/progress_phase_5.md) (released [v0.5.0](https://github.com/goglesquirmintontheiii/us-explorer/releases/tag/v0.5.0))

Goal: fix the black, unresponsive screen at start in 0.5.0. The 0.5.0 checklist carries over to this release for testing. Linux AppImage and Windows build.

## Tasks

- [x] Black screen with no menus at start: the menus (egui) attached themselves to whichever camera was created first, and in 0.5.0 that could be the bathroom mirror's camera, which renders to a hidden image and is off. The menus now always draw on the main camera
- [x] Roadside objects (signals, mailboxes, hydrants, cars...) sometimes appeared enormous right on top of you: Bevy's static-scene shortcut in transform propagation left some newly placed objects at the world origin. Transforms are now propagated in full every frame
- [x] The info overlay and minimap no longer show over the title screen
- [x] The scripted screenshot runs now include the title screen (`menu=title`), so a blank menu is caught by CI
- [x] v0.5.1-hotfix released (AppImage and Windows .exe)

## How it was found

Reproduced locally by starting the game normally under a virtual display (the scripted screenshot runs used for checking 0.5.0 skip the title screen, so they never showed it), then clicking through the title screen with synthetic mouse input and logging which objects had stale positions.
