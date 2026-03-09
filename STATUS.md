# Anno 1602 RE — Project Status

## Reverse Engineering

| Subsystem | Functions | Coverage |
|-----------|-----------|----------|
| Graphics engine | ~50 key functions mapped | Full architecture understood: DirectDraw/GDI dual backend, BSH sprite format, isometric renderer, palette system |
| Sound engine | 70/70 functions | Fully analyzed: wave slots, streaming, ADPCM codec, 3D positioning |
| Game simulation | 12 subsystem tickers identified | Core formulas extracted: production efficiency, population happiness, tax/bankruptcy, entity state machine |
| AI controller | Entry point identified | Not yet deeply analyzed |
| Networking (Maxnet.dll) | Not started | 160KB, not yet decompiled |
| Save/load | Entry point identified | Format documented by community, not yet implemented |

## Rust Implementation

| Crate | Status | What works |
|-------|--------|------------|
| `anno-formats` | BSH parser verified | Parses 5964 sprites from STADTFLD.BSH correctly. COD parser is skeleton (encryption key needs verification). |
| `anno-audio` | ADPCM codec tested | Encode/decode roundtrip passes. Wave/stream managers are structurally complete but untested with real audio. |
| `anno-render` | Compiles, not tested | Framebuffer blitter, isometric camera, tile renderer — all written but no visual output yet. |
| `anno-sim` | Compiles, not tested | Production, economy, entity types defined. Simulation dispatcher structured. All tick handlers are stubs. |
| `anno-game` | Skeleton | Empty integration crate. |

### What's missing to actually run something

1. No palette loaded — need to extract the 256-color palette from the game binary or data files
2. No SDL2 window — the render crate has no main loop or display surface yet
3. No building definitions loaded — `haeuser.cod` parsing not wired up
4. No map/island loading — save file or scenario parsing not implemented

## Next Steps

### Phase 1: Show something on screen
1. Extract palette from the game binary (at `DAT_0072b280`) or from a save file
2. Wire up SDL2 — create a window, blit the framebuffer to a texture, handle input
3. Render sprites — load STADTFLD.BSH, draw individual sprites to verify the BSH parser + RLE decoder visually
4. Render a test island — load a save file, parse island tile data, render with the isometric camera

### Phase 2: Game data pipeline
5. Fix COD parser — verify the encryption scheme against real `haeuser.cod`, parse building definitions
6. Parse save files — the `.szs` format is community-documented, implement a loader
7. Load all sprite sets — GFX/MGFX/SGFX at three zoom levels

### Phase 3: Simulation
8. Production chains — wire building defs to the production ticker, test with a small scenario
9. Entity movement — implement pathfinding (likely A* on the iso grid) and carrier dispatching
10. Population model — connect demand/supply tracking to warehouse inventories

### Phase 4: Deeper RE
11. AI analysis — deep dive into `FUN_0042b4b0` and its sub-functions for AI decision trees
12. Multiplayer protocol — decompile `Maxnet.dll` if multiplayer is a goal
13. Combat formulas — extract damage calculations and unit stats from the military subsystem
