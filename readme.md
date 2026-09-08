# Rust WASM Physics Core for Cloudflare Actors

This repository contains the high-performance Rust core used to power the physics and game logic for a Cloudflare Actors-based real-time multiplayer game. It is designed to be compiled to WebAssembly (WASM) and provides lightning-fast simulation using SIMD and zero-copy memory interop.

## 🚀 Key Features

*   **Zero-Copy Memory Interop:** Uses `bytemuck` to expose raw memory pointers directly to JavaScript/TypeScript, completely bypassing expensive JSON serialization.
*   **SIMD Bullet Processing:** Utilizes `ultraviolet` to process bullets in batches of 4 simultaneously, maximizing CPU efficiency for mass projectile updates.
*   **Deterministic 2D Physics:** Powered by `rapier2d`, featuring rigid body dynamics, Continuous Collision Detection (CCD), and linear damping.
*   **Vector Math & Aiming:** Uses `glam` for efficient 2D vector calculations, easily handling player rotation and aiming logic.
*   **Auto-Generated TypeScript Types:** Uses `ts-rs` to automatically generate the `PlayerState.ts` definitions straight from the Rust struct, ensuring perfect frontend/backend sync.

## 📦 Core Dependencies

*   **`rapier2d`**: Core 2D physics engine (Colliders, RigidBodies, Tick integration).
*   **`ultraviolet` & `wide`**: SIMD operations for ultra-fast projectile math.
*   **`glam`**: Lightweight linear algebra (used for aiming math).
*   **`bytemuck`**: Casting structs to raw byte slices (Pod, Zeroable) for WASM bridging.
*   **`ts-rs`**: Bridging Rust types to TypeScript.

## 🛠️ Exposed WASM API (FFI)

The following functions are exposed to the JS/TS Cloudflare Actor environment via C-ABI (`extern "C"`):

*   **`world_create(max_players, max_bullets)`**
    Initializes the Rapier physics world, allocates vectors, and aligns memory for SIMD chunking.
*   **`add_static_wall(w_ptr, x, y, hw, hh)`**
    Spawns static cuboid colliders (used for arena boundaries).
*   **`spawn_player(w_ptr, id, x, y)`**
    Creates a dynamic rigid body ball collider for a player, enabling Continuous Collision Detection (CCD).
*   **`player_input_and_aim(w_ptr, id, move_vx, move_vy, aim_target_x, aim_target_y)`**
    Updates player linear velocity and automatically computes the correct aiming rotation angle using `glam`.
*   **`process_bullets_simd(w_ptr, dt)`**
    Processes active bullets in SIMD chunks (4 at a time). Updates positions and efficiently culls bullets that leave the 800x600 arena bounds.
*   **`world_tick(w_ptr, dt)`**
    Steps the Rapier physics pipeline forward and maps the results into a flat `PlayerState` array. Returns the number of active players.
*   **`get_players_ptr(w_ptr)`**
    Returns a direct C-pointer (`*const u8`) to the packed `PlayerState` array for instantaneous read access by the Cloudflare Actor. 

## 💾 Memory Layout (PlayerState)

The `PlayerState` struct is strictly packed into **32 bytes** to ensure highly efficient memory reads by the JavaScript Host:
*   `id` (u32)
*   `_pad` (u32 - Memory Alignment Padding)
*   `x`, `y` (f32)
*   `vx`, `vy` (f32)
*   `rotation` (f32)
*   `health` (f32)
