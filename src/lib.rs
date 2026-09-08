use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use rapier2d::prelude::*;
use ts_rs::TS;
use typeshare::typeshare;
use ultraviolet::f32x4;
use wide::{CmpGt, CmpLt};
use interoptopus::{ffi_type, ffi_function, Inventory, InventoryBuilder, function};

// =====================================================================
// 1. DATA LAYOUTS (Zero-Copy Multi-Lang Setup)
// =====================================================================

/// 32-byte packed struct for Server -> Client (State)
#[repr(C)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, Pod, Zeroable, TS)]
pub struct PlayerState {
    pub id: u32,
    pub _pad: u32,
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub rotation: f32,
    pub health: f32,
}

/// 16-byte aligned struct for Client -> Server (Input)
#[repr(C)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, Pod, Zeroable, TS)]
pub struct PlayerInput {
    pub msg_type: u8,   // Offset 0 (1 byte)
    pub dir_code: u8,   // Offset 1 (1 byte)
    pub _pad: u16,      // Offset 2 (2 bytes) -> Padding to reach 4-byte boundary
    pub seq: u32,       // Offset 4 (4 bytes)
    pub timestamp: f64, // Offset 8 (8 bytes) -> Total 16 bytes
}

#[ffi_function]
#[no_mangle]
pub extern "C" fn _export_player_state_for_sdk(_p: PlayerState) {}

#[ffi_function]
#[no_mangle]
pub extern "C" fn _export_player_input_for_sdk(_p: PlayerInput) {}

// =====================================================================
// 2. WORLD STRUCTURE
// =====================================================================

#[ffi_type(opaque)]
pub struct GameWorld {
    pub integration_parameters: IntegrationParameters,
    pub physics_pipeline: PhysicsPipeline,
    pub island_manager: IslandManager,
    pub broad_phase: DefaultBroadPhase,
    pub narrow_phase: NarrowPhase,
    pub bodies: RigidBodySet,
    pub colliders: ColliderSet,
    pub impulse_joints: ImpulseJointSet,
    pub multibody_joints: MultibodyJointSet,
    pub ccd_solver: CCDSolver,

    pub tracked_players: Vec<(u32, RigidBodyHandle)>,
    pub player_snapshots: Vec<PlayerState>,

    pub max_bullets: usize,
    pub bul_active: Vec<u32>,
    pub bul_x: Vec<f32>,
    pub bul_y: Vec<f32>,
    pub bul_vx: Vec<f32>,
    pub bul_vy: Vec<f32>,
}

// =====================================================================
// 3. LIFECYCLE & RAPIER INITIALIZATION
// =====================================================================

#[ffi_function]
#[no_mangle]
pub extern "C" fn world_create(max_players: u32, max_bullets: u32) -> *mut GameWorld {
    let b_cap = ((max_bullets + 3) & !3) as usize; 
    let mut integration_parameters = IntegrationParameters::default();
    integration_parameters.dt = 1.0 / 60.0;

    let world = Box::new(GameWorld {
        integration_parameters,
        physics_pipeline: PhysicsPipeline::new(),
        island_manager: IslandManager::new(),
        broad_phase: DefaultBroadPhase::new(),
        narrow_phase: NarrowPhase::new(),
        bodies: RigidBodySet::new(),
        colliders: ColliderSet::new(),
        impulse_joints: ImpulseJointSet::new(),
        multibody_joints: MultibodyJointSet::new(),
        ccd_solver: CCDSolver::new(),

        tracked_players: Vec::with_capacity(max_players as usize),
        player_snapshots: Vec::with_capacity(max_players as usize),

        max_bullets: b_cap,
        bul_active: vec![0; b_cap],
        bul_x: vec![0.0; b_cap],
        bul_y: vec![0.0; b_cap],
        bul_vx: vec![0.0; b_cap],
        bul_vy: vec![0.0; b_cap],
    });
    Box::into_raw(world)
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn add_static_wall(w_ptr: *mut GameWorld, x: f32, y: f32, hw: f32, hh: f32) {
    let w = &mut *w_ptr;
    let collider = ColliderBuilder::cuboid(hw, hh)
        .translation(vector![x, y].into())
        .build();
    w.colliders.insert(collider);
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn spawn_player(w_ptr: *mut GameWorld, id: u32, x: f32, y: f32) {
    let w = &mut *w_ptr;
    let body = RigidBodyBuilder::dynamic()
        .translation(vector![x, y].into())
        .linear_damping(5.0)
        .ccd_enabled(true)
        .build();
    let collider = ColliderBuilder::ball(10.0).restitution(0.2).build();
    let handle = w.bodies.insert(body);
    w.colliders.insert_with_parent(collider, handle, &mut w.bodies);
    w.tracked_players.push((id, handle));
}

// =====================================================================
// 4. GLAM: PLAYER AIMING & VECTOR MATH
// =====================================================================

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn player_input_and_aim(
    w_ptr: *mut GameWorld, id: u32, move_vx: f32, move_vy: f32, aim_target_x: f32, aim_target_y: f32,
) {
    let w = &mut *w_ptr;
    if let Some(&(_, handle)) = w.tracked_players.iter().find(|(pid, _)| *pid == id) {
        if let Some(body) = w.bodies.get_mut(handle) {
            body.set_linvel(vector![move_vx, move_vy].into(), true);
            let pos = body.translation();
            let current_pos = Vec2::new(pos.x, pos.y);
            let target_pos = Vec2::new(aim_target_x, aim_target_y);
            let direction = target_pos - current_pos;
            if direction.length_squared() > 0.1 {
                let angle = direction.y.atan2(direction.x);
                body.set_rotation(Rotation::new(angle), true);
            }
        }
    }
}

// =====================================================================
// 5. ULTRAVIOLET: MASS BULLET SIMD
// =====================================================================

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn process_bullets_simd(w_ptr: *mut GameWorld, dt: f32) {
    let w = &mut *w_ptr;
    let chunks = w.max_bullets / 4;
    let dt_vec = f32x4::splat(dt);

    for i in 0..chunks {
        let idx = i * 4;
        let active = &w.bul_active[idx..idx+4];
        if active[0] == 0 && active[1] == 0 && active[2] == 0 && active[3] == 0 { continue; } 

        let mut px = f32x4::from(<[f32; 4]>::try_from(&w.bul_x[idx..idx+4]).unwrap());
        let mut py = f32x4::from(<[f32; 4]>::try_from(&w.bul_y[idx..idx+4]).unwrap());
        let vx = f32x4::from(<[f32; 4]>::try_from(&w.bul_vx[idx..idx+4]).unwrap());
        let vy = f32x4::from(<[f32; 4]>::try_from(&w.bul_vy[idx..idx+4]).unwrap());

        px += vx * dt_vec;
        py += vy * dt_vec;

        let out_x = px.cmp_lt(f32x4::splat(0.0)) | px.cmp_gt(f32x4::splat(800.0));
        let out_y = py.cmp_lt(f32x4::splat(0.0)) | py.cmp_gt(f32x4::splat(600.0));
        let is_out = out_x | out_y;

        w.bul_x[idx..idx+4].copy_from_slice(&<[f32; 4]>::from(px));
        w.bul_y[idx..idx+4].copy_from_slice(&<[f32; 4]>::from(py));

        let out_mask: [u32; 4] = core::mem::transmute(is_out);
        for k in 0..4 {
            if out_mask[k] != 0 { w.bul_active[idx + k] = 0; }
        }
    }
}

// =====================================================================
// 6. RAPIER PHYSICS TICK & SNAPSHOT
// =====================================================================

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn world_tick(w_ptr: *mut GameWorld, dt: f32) -> u32 {
    let w = &mut *w_ptr;
    w.integration_parameters.dt = dt;

    w.physics_pipeline.step(
        vector![0.0, 0.0].into(),
        &w.integration_parameters,
        &mut w.island_manager,
        &mut w.broad_phase,
        &mut w.narrow_phase,
        &mut w.bodies,
        &mut w.colliders,
        &mut w.impulse_joints,
        &mut w.multibody_joints,
        &mut w.ccd_solver,
        &(), 
        &(), 
    );

    w.player_snapshots.clear();
    for &(id, handle) in &w.tracked_players {
        if let Some(body) = w.bodies.get(handle) {
            let pos = body.translation();
            let vel = body.linvel();
            let rot = body.rotation().angle();

            w.player_snapshots.push(PlayerState {
                id, _pad: 0, x: pos.x, y: pos.y, vx: vel.x, vy: vel.y, rotation: rot, health: 100.0,
            });
        }
    }
    w.player_snapshots.len() as u32
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn get_players_ptr(w: *mut GameWorld) -> *const u8 {
    bytemuck::cast_slice(&(*w).player_snapshots).as_ptr()
}

// Expose inventory so the generator binary can read it
pub fn my_inventory() -> Inventory {
    InventoryBuilder::new()
        .register(function!(_export_player_state_for_sdk))
        .register(function!(_export_player_input_for_sdk))
        .register(function!(world_create))
        .register(function!(add_static_wall))
        .register(function!(spawn_player))
        .register(function!(player_input_and_aim))
        .register(function!(process_bullets_simd))
        .register(function!(world_tick))
        .register(function!(get_players_ptr))
        .inventory()
}
