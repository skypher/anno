# Anno 1602 Reverse Engineering Notes

## Binary Overview
- **1602.exe**: 632KB PE32, 1719 functions decompiled
- **Maxsound.dll**: 32KB, 70 functions (sound engine)
- **Maxnet.dll**: 160KB (networking, not yet analyzed)

## Key Addresses

### Main Loop
- `FUN_004081d0`: Main game loop (message pump + idle dispatch)
- `FUN_00492410`: Video/cutscene controller
- `FUN_00495a00`: Main render frame
- `FUN_004897b0`: Main scene renderer
- `FUN_00489670`: Simulation dispatcher

### Graphics
- `DAT_005b3104/08/0c/10`: DirectDraw objects
- `DAT_005b2ce8`: Rendering mode (0=GDI, 1=DirectDraw)
- `DAT_0072c080`: Render target pointer
- `PTR_DAT_0049af64`: Camera/viewport structure
- `DAT_0049d778`: Sprite table (BSH sets)
- `DAT_0072b280`: Palette data (256 entries)

### Sprite Functions
- `FUN_00401b10`: Draw sprite (normal)
- `FUN_00401bf0`: Draw sprite with remap
- `FUN_00401d10`: Draw sprite with overlay
- `FUN_00401d90`: Get sprite dimensions
- `FUN_00403670`: Raw bitmap blitter
- `FUN_00403920`: Remapped pixel copy (unrolled)
- `FUN_00403de0`: RLE sprite blitter

### Isometric Rendering
- `FUN_00489a60`: Main tile row renderer
- `FUN_00489c90`: Per-tile renderer
- `FUN_0048a140`: Sprite rendering dispatch
- `FUN_004862c0`: Zoom level handler
- `FUN_004863f0`: Set camera position
- `FUN_004861c0`: Scroll-to-tile conversion

### Simulation Subsystems (12 tickers in FUN_00489670)
1. `FUN_0047ca80`: Tile animation (40000ms)
2. `FUN_0047daf0`: Building production (999ms)
3. `FUN_0047f8a0`: Population/economy (9999ms)
4. `FUN_0047b9c0`: Citizen movement (15000ms)
5. `FUN_0046b3e0`: Island-level events (29999ms)
6. `FUN_00478ab0`: Event/quest scheduler
7. `FUN_004791b0`: Ship movement (1000ms)
8. `FUN_004798e0`: Multi-slot building (1000ms)
9. `FUN_0047a020`: Military/settler (9999ms)
10. `FUN_0047a8c0`: Projectile (9999ms)
11. `FUN_00476350`: Diplomacy/economy (4999ms)
12. `FUN_0042b4b0`: AI controller

### Entity System
- `FUN_00451890`: Figure action state machine (2550 entities, 18+ action types)

### Key Data Tables
- `DAT_00619b60`: Building definitions (136 bytes each)
- `DAT_005b7680`: Player data (160 bytes each, 7 players)
- `DAT_005e6b20`: Island data (2816 bytes each, 50 islands)
- `PTR_DAT_0049aebc`: Active buildings (20 bytes each, 1037 max)

### Game Constants
- 600 game ticks = 1 displayed minute
- Game speed multiplier at `DAT_0049af74`
- Auto-save every 599999ms
- Bankruptcy at -1001 gold, game over after 40 ticks bankrupt
- Satisfaction scale: 0-128 (128 = 100%)
- Satisfaction decay: 15/16 per economy tick
