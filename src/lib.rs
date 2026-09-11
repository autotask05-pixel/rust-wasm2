use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use interoptopus::{ffi_function, ffi_type, function, Inventory, InventoryBuilder};
use ts_rs::TS;
use typeshare::typeshare;
use ultraviolet::f32x4;

// =====================================================================
// 1. CROSS-LANGUAGE DATA LAYOUTS (Zero-Copy & SIMD Aligned)
// =====================================================================

pub const TEAM_BLUE: u8 = 0;
pub const TEAM_RED: u8 = 1;
pub const NO_CARRIER: u32 = u32::MAX;

#[repr(u8)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BrawlerId {
    Shelly = 0,
    Colt = 1,
    ElPrimo = 2,
    Mortis = 3,
    Spike = 4,
}

#[repr(C)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, Pod, Zeroable, TS)]
pub struct PlayerInput {
    pub seq: u32,
    pub move_x: f32,
    pub move_y: f32,
    pub aim_x: f32,
    pub aim_y: f32,
    pub attack: u8, // 0 = None, 1 = Normal, 2 = Super
    pub _pad: [u8; 3],
}

#[repr(C)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, Pod, Zeroable, TS)]
pub struct PlayerSnapshot {
    pub id: u32,
    pub team: u8,
    pub brawler_id: u8,
    pub has_ball: u8,
    pub _pad1: u8,
    pub x: f32,
    pub y: f32,
    pub rotation: f32,
    pub health_ratio: f32, // 0.0 to 1.0
    pub ammo: f32,         // 0.0 to 3.0
    pub super_ratio: f32,  // 0.0 to 1.0
}

#[repr(C)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, Pod, Zeroable, TS)]
pub struct BallSnapshot {
    pub x: f32,
    pub y: f32,
    pub vx: f32,
    pub vy: f32,
    pub carrier_id: u32,
}

#[repr(C)]
#[ffi_type]
#[typeshare]
#[derive(Clone, Copy, Pod, Zeroable, TS)]
pub struct MatchSnapshot {
    pub blue_score: u32,
    pub red_score: u32,
    pub phase: u32, // 0 = Active, 1 = Goal Pause, 2 = Over
    pub timer: f32,
}

// Dummy export to register data structures with Interoptopus SDK generator
#[ffi_function]
#[no_mangle]
pub extern "C" fn _export_models(_p: PlayerSnapshot, _b: BallSnapshot, _m: MatchSnapshot, _i: PlayerInput, _e: BrawlerId) {}

// =====================================================================
// 2. DESTRUCTIBLE TILE MAP & PHYSICS
// =====================================================================

pub const TILE_SIZE: f32 = 32.0;
pub const MAP_COLS: usize = 22;
pub const MAP_ROWS: usize = 34;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TileType { Empty, Wall, DestructibleWall, GoalBlue, GoalRed }

pub struct DestructibleGrid {
    pub tiles: Vec<TileType>,
}

impl DestructibleGrid {
    pub fn new_brawl_ball() -> Self {
        let mut tiles = vec![TileType::Empty; MAP_COLS * MAP_ROWS];
        for r in 0..MAP_ROWS {
            for c in 0..MAP_COLS {
                let idx = r * MAP_COLS + c;
                if r == 0 || r == MAP_ROWS - 1 {
                    tiles[idx] = if c >= 8 && c <= 13 {
                        if r == 0 { TileType::GoalBlue } else { TileType::GoalRed }
                    } else { TileType::Wall };
                } else if c == 0 || c == MAP_COLS - 1 {
                    tiles[idx] = TileType::Wall;
                }
            }
        }
        // Classic Brawl Ball destructible mid-field barriers
        let barriers = [(7, 10), (8, 10), (13, 10), (14, 10), (7, 23), (8, 23), (13, 23), (14, 23)];
        for (c, r) in barriers {
            tiles[r * MAP_COLS + c] = TileType::DestructibleWall;
        }
        Self { tiles }
    }

    pub fn is_solid(&self, p: Vec2) -> bool {
        let c = (p.x / TILE_SIZE) as usize;
        let r = (p.y / TILE_SIZE) as usize;
        if c >= MAP_COLS || r >= MAP_ROWS { return true; }
        matches!(self.tiles[r * MAP_COLS + c], TileType::Wall | TileType::DestructibleWall)
    }

    pub fn destroy_radius(&mut self, center: Vec2, radius: f32) {
        for r in 0..MAP_ROWS {
            for c in 0..MAP_COLS {
                let pos = Vec2::new(c as f32 * TILE_SIZE + 16.0, r as f32 * TILE_SIZE + 16.0);
                let idx = r * MAP_COLS + c;
                if (pos - center).length_squared() <= radius * radius {
                    if self.tiles[idx] == TileType::DestructibleWall {
                        self.tiles[idx] = TileType::Empty;
                    }
                }
            }
        }
    }
}

// =====================================================================
// 3. SIMD PROJECTILE PIPELINE
// =====================================================================

pub struct ProjectileFlags;
impl ProjectileFlags {
    pub const NORMAL: u32 = 0;
    pub const WALL_BREAKER: u32 = 1 << 0;
    pub const PIERCING: u32 = 1 << 1;
    pub const KNOCKBACK: u32 = 1 << 2;
    pub const LIFESTEAL: u32 = 1 << 3;
    pub const EXPLODE: u32 = 1 << 4;
}

pub struct ProjectileManager {
    pub count: usize,
    pub x: Vec<f32>, pub y: Vec<f32>,
    pub vx: Vec<f32>, pub vy: Vec<f32>,
    pub life: Vec<f32>, pub dmg: Vec<f32>, pub radius: Vec<f32>,
    pub owner: Vec<usize>, pub flags: Vec<u32>,
}

impl ProjectileManager {
    pub fn new(cap: usize) -> Self {
        let align_cap = (cap + 3) & !3; // Pad to multiple of 4 for f32x4 SIMD lanes
        Self {
            count: 0,
            x: vec![0.0; align_cap], y: vec![0.0; align_cap],
            vx: vec![0.0; align_cap], vy: vec![0.0; align_cap],
            life: vec![0.0; align_cap], dmg: vec![0.0; align_cap], radius: vec![0.0; align_cap],
            owner: vec![0; align_cap], flags: vec![0; align_cap],
        }
    }

    pub fn spawn(&mut self, pos: Vec2, vel: Vec2, life: f32, dmg: f32, rad: f32, owner: usize, flag: u32) {
        if self.count >= self.x.len() { return; }
        let i = self.count;
        self.x[i] = pos.x; self.y[i] = pos.y;
        self.vx[i] = vel.x; self.vy[i] = vel.y;
        self.life[i] = life; self.dmg[i] = dmg; self.radius[i] = rad;
        self.owner[i] = owner; self.flags[i] = flag;
        self.count += 1;
    }

    pub fn tick_simd(&mut self, dt: f32, grid: &mut DestructibleGrid) {
        let dt_vec = f32x4::splat(dt);
        let chunks = (self.count + 3) / 4;

        for c in 0..chunks {
            let idx = c * 4;
            let mut px = f32x4::from(<[f32; 4]>::try_from(&self.x[idx..idx+4]).unwrap());
            let mut py = f32x4::from(<[f32; 4]>::try_from(&self.y[idx..idx+4]).unwrap());
            let vx = f32x4::from(<[f32; 4]>::try_from(&self.vx[idx..idx+4]).unwrap());
            let vy = f32x4::from(<[f32; 4]>::try_from(&self.vy[idx..idx+4]).unwrap());
            let mut l = f32x4::from(<[f32; 4]>::try_from(&self.life[idx..idx+4]).unwrap());

            px += vx * dt_vec; py += vy * dt_vec; l -= dt_vec;

            self.x[idx..idx+4].copy_from_slice(&<[f32; 4]>::from(px));
            self.y[idx..idx+4].copy_from_slice(&<[f32; 4]>::from(py));
            self.life[idx..idx+4].copy_from_slice(&<[f32; 4]>::from(l));
        }

        let mut i = 0;
        while i < self.count {
            let pos = Vec2::new(self.x[i], self.y[i]);
            let is_dead = self.life[i] <= 0.0;
            let hit_wall = grid.is_solid(pos);

            if is_dead || hit_wall {
                if (self.flags[i] & ProjectileFlags::WALL_BREAKER) != 0 {
                    grid.destroy_radius(pos, 48.0); // Shatters wall!
                }
                
                if (self.flags[i] & ProjectileFlags::EXPLODE) != 0 {
                    // Spike Grenade Split
                    let owner = self.owner[i];
                    for k in 0..6 {
                        let rad = (k as f32) * (core::f32::consts::PI / 3.0);
                        let dir = Vec2::new(rad.cos(), rad.sin());
                        self.spawn(pos, dir * 750.0, 0.25, 520.0, 6.0, owner, ProjectileFlags::NORMAL);
                    }
                }
                self.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }

    pub fn swap_remove(&mut self, i: usize) {
        self.count -= 1;
        let last = self.count;
        self.x[i] = self.x[last]; self.y[i] = self.y[last];
        self.vx[i] = self.vx[last]; self.vy[i] = self.vy[last];
        self.life[i] = self.life[last]; self.dmg[i] = self.dmg[last];
        self.radius[i] = self.radius[last];
        self.owner[i] = self.owner[last]; self.flags[i] = self.flags[last];
    }
}

// =====================================================================
// 4. GAME ENGINE & BRAWLER MECHANICS
// =====================================================================

#[derive(Clone, Copy)]
pub struct ActiveAction {
    pub action_type: u8, // 1 = Colt Burst, 2 = Primo Leap, 3 = Mortis Dash
    pub timer: f32,
    pub step: u8,
    pub dir: Vec2,
}

pub struct Player {
    pub id: u32, pub team: u8, pub brawler: BrawlerId,
    pub pos: Vec2, pub vel: Vec2, pub rot: f32,
    pub hp: f32, pub max_hp: f32, pub speed: f32,
    pub ammo: f32, pub reload_rate: f32, pub super_charge: f32,
    pub respawn: f32, pub out_of_combat: f32,
    pub is_airborne: bool, pub action: Option<ActiveAction>,
}

#[ffi_type(opaque)]
pub struct Engine {
    pub players: Vec<Player>,
    pub projs: ProjectileManager,
    pub grid: DestructibleGrid,
    
    // Brawl Ball Specifics
    pub ball_pos: Vec2, pub ball_vel: Vec2,
    pub ball_carrier: usize, pub kick_cooldown: f32,
    pub score_blue: u32, pub score_red: u32, 
    pub phase: u32, pub timer: f32,

    pub snapshots: Vec<PlayerSnapshot>,
    pub ball_snap: BallSnapshot,
    pub match_snap: MatchSnapshot,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            players: Vec::new(), projs: ProjectileManager::new(1024),
            grid: DestructibleGrid::new_brawl_ball(),
            ball_pos: Vec2::new(352.0, 544.0), ball_vel: Vec2::ZERO,
            ball_carrier: usize::MAX, kick_cooldown: 0.0,
            score_blue: 0, score_red: 0, phase: 0, timer: 150.0,
            snapshots: Vec::new(),
            ball_snap: BallSnapshot { x: 0.0, y: 0.0, vx: 0.0, vy: 0.0, carrier_id: NO_CARRIER },
            match_snap: MatchSnapshot { blue_score: 0, red_score: 0, phase: 0, timer: 150.0 }
        }
    }

    pub fn spawn_player(&mut self, id: u32, team: u8, brawler: BrawlerId, pos: Vec2) {
        let (max_hp, speed, reload) = match brawler {
            BrawlerId::Shelly => (5400.0, 220.0, 0.67), // Normal speed, 1.5s reload
            BrawlerId::Colt => (4200.0, 220.0, 0.62),   // Normal speed, 1.6s reload
            BrawlerId::ElPrimo => (7800.0, 250.0, 1.25),// Fast speed, 0.8s reload
            BrawlerId::Mortis => (5600.0, 260.0, 0.41), // Very Fast, 2.4s reload
            BrawlerId::Spike => (3800.0, 210.0, 0.50),  // Slow speed, 2.0s reload
        };
        
        self.players.push(Player {
            id, team, brawler, pos, vel: Vec2::ZERO, rot: 0.0,
            hp: max_hp, max_hp, speed, ammo: 3.0, reload_rate: reload,
            super_charge: 0.0, respawn: 0.0, out_of_combat: 0.0,
            is_airborne: false, action: None,
        });
        self.snapshots.push(PlayerSnapshot { id, team, brawler_id: brawler as u8, has_ball: 0, _pad1: 0, x: pos.x, y: pos.y, rotation: 0.0, health_ratio: 1.0, ammo: 3.0, super_ratio: 0.0 });
    }

    pub fn tick(&mut self, dt: f32) {
        if self.phase == 1 {
            self.timer -= dt; 
            if self.timer <= 0.0 { self.reset_pitch(); self.phase = 0; }
            return;
        } else if self.phase == 2 {
            return; // Game over
        }

        self.timer = (self.timer - dt).max(0.0);
        if self.timer <= 0.0 { self.phase = 2; }

        self.projs.tick_simd(dt, &mut self.grid);
        if self.kick_cooldown > 0.0 { self.kick_cooldown -= dt; }

        // Process Players
        for i in 0..self.players.len() {
            let p = &mut self.players[i];
            if p.respawn > 0.0 { 
                p.respawn -= dt; 
                if p.respawn <= 0.0 { p.hp = p.max_hp; p.pos = Self::get_spawn(p.team); }
                continue; 
            }

            p.ammo = (p.ammo + p.reload_rate * dt).min(3.0);
            p.out_of_combat += dt;
            if p.out_of_combat > 3.0 { p.hp = (p.hp + p.max_hp * 0.13 * dt).min(p.max_hp); }

            // Channeled Actions (Colt Burst, Mortis Dash, Primo Leap)
            if let Some(mut act) = p.action {
                act.timer -= dt;
                match act.action_type {
                    1 => { // Colt Burst
                        if act.timer <= 0.0 && act.step < 6 {
                            self.projs.spawn(p.pos, act.dir * 1250.0, 0.7, 520.0, 8.0, i, ProjectileFlags::NORMAL);
                            act.step += 1; act.timer = 0.1;
                        }
                    },
                    2 => { // El Primo Leap
                        p.is_airborne = true;
                        p.vel = act.dir * 600.0; // Fly towards target
                        if act.timer <= 0.0 {
                            p.is_airborne = false;
                            p.vel = Vec2::ZERO;
                            self.grid.destroy_radius(p.pos, 48.0);
                            self.projs.spawn(p.pos, Vec2::ZERO, 0.1, 1200.0, 60.0, i, ProjectileFlags::KNOCKBACK);
                        }
                    },
                    3 => { // Mortis Dash
                        p.vel = act.dir * 900.0;
                    }
                    _ => {}
                }
                if act.timer <= 0.0 && act.step >= 6 || (act.action_type != 1 && act.timer <= 0.0) {
                    p.action = None;
                    if act.action_type == 3 { p.vel = Vec2::ZERO; } // Stop Mortis dash
                } else {
                    p.action = Some(act);
                }
            }

            // Movement & Wall Collisions
            if !p.is_airborne {
                let next = p.pos + p.vel * dt;
                // Simple Circle-AABB Wall Sliding
                if !self.grid.is_solid(Vec2::new(next.x, p.pos.y)) { p.pos.x = next.x; }
                if !self.grid.is_solid(Vec2::new(p.pos.x, next.y)) { p.pos.y = next.y; }
            } else {
                p.pos += p.vel * dt;
            }
        }

        self.resolve_projectile_hits();
        self.tick_ball(dt);

        // Snapshots
        for i in 0..self.players.len() {
            let p = &self.players[i];
            self.snapshots[i] = PlayerSnapshot {
                id: p.id, team: p.team, brawler_id: p.brawler as u8,
                has_ball: if self.ball_carrier == i { 1 } else { 0 }, _pad1: 0,
                x: p.pos.x, y: p.pos.y, rotation: p.rot,
                health_ratio: (p.hp / p.max_hp).clamp(0.0, 1.0),
                ammo: p.ammo, super_ratio: (p.super_charge / 100.0).clamp(0.0, 1.0),
            };
        }
        self.ball_snap = BallSnapshot { x: self.ball_pos.x, y: self.ball_pos.y, vx: self.ball_vel.x, vy: self.ball_vel.y, carrier_id: if self.ball_carrier != usize::MAX { self.players[self.ball_carrier].id } else { NO_CARRIER } };
        self.match_snap = MatchSnapshot { blue_score: self.score_blue, red_score: self.score_red, phase: self.phase, timer: self.timer };
    }

    fn resolve_projectile_hits(&mut self) {
        let mut i = 0;
        while i < self.projs.count {
            let p_pos = Vec2::new(self.projs.x[i], self.projs.y[i]);
            let p_owner = self.projs.owner[i];
            let owner_team = self.players[p_owner].team;
            let rad = self.projs.radius[i];
            let dmg = self.projs.dmg[i];
            let flags = self.projs.flags[i];
            
            let mut hit_target = false;
            let mut super_charge_gained = 0.0;
            let mut lifesteal_gained = 0.0;

            for t_idx in 0..self.players.len() {
                let target = &mut self.players[t_idx];
                if target.team == owner_team || target.hp <= 0.0 || target.is_airborne { continue; }

                if (target.pos - p_pos).length_squared() < (rad + 16.0).powi(2) {
                    target.hp -= dmg;
                    target.out_of_combat = 0.0;
                    hit_target = true;

                    // Accumulate stats for the attacker instead of borrowing them directly here
                    super_charge_gained += 15.0;
                    if (flags & ProjectileFlags::LIFESTEAL) != 0 {
                        lifesteal_gained += dmg;
                    }

                    // Knockback strips ball!
                    if (flags & ProjectileFlags::KNOCKBACK) != 0 {
                        if self.ball_carrier == t_idx {
                            self.ball_carrier = usize::MAX;
                            self.ball_vel = (target.pos - p_pos).normalize_or_zero() * 400.0;
                        }
                    }

                    if target.hp <= 0.0 {
                        target.respawn = 5.0; // 5s respawn
                        if self.ball_carrier == t_idx { self.ball_carrier = usize::MAX; }
                    }
                }
            }

            // Apply the deferred stat gains to the attacker
            if super_charge_gained > 0.0 || lifesteal_gained > 0.0 {
                let attacker = &mut self.players[p_owner];
                attacker.super_charge = (attacker.super_charge + super_charge_gained).min(100.0);
                if lifesteal_gained > 0.0 {
                    attacker.hp = (attacker.hp + lifesteal_gained).min(attacker.max_hp);
                }
            }

            if hit_target && (flags & ProjectileFlags::PIERCING) == 0 {
                self.projs.swap_remove(i);
            } else { i += 1; }
        }
    }

    fn tick_ball(&mut self, dt: f32) {
        if self.ball_carrier != usize::MAX {
            let p = &self.players[self.ball_carrier];
            if p.hp <= 0.0 || p.is_airborne {
                self.ball_carrier = usize::MAX;
            } else {
                let dir = Vec2::new(p.rot.cos(), p.rot.sin());
                self.ball_pos = p.pos + dir * 20.0; // Snap to front of carrier
                self.ball_vel = Vec2::ZERO;
            }
        } else {
            // Free rolling ball physics
            let next = self.ball_pos + self.ball_vel * dt;
            if self.grid.is_solid(Vec2::new(next.x, self.ball_pos.y)) { self.ball_vel.x *= -0.8; } else { self.ball_pos.x = next.x; }
            if self.grid.is_solid(Vec2::new(self.ball_pos.x, next.y)) { self.ball_vel.y *= -0.8; } else { self.ball_pos.y = next.y; }
            
            self.ball_vel *= 0.96; // Rolling friction

            // Possession Pickup
            if self.kick_cooldown <= 0.0 {
                for (i, p) in self.players.iter().enumerate() {
                    if p.hp > 0.0 && !p.is_airborne && (p.pos - self.ball_pos).length_squared() < 24.0 * 24.0 {
                        self.ball_carrier = i; break;
                    }
                }
            }
        }

        // Goal Detection
        let t_c = (self.ball_pos.x / TILE_SIZE) as usize;
        let t_r = (self.ball_pos.y / TILE_SIZE) as usize;
        if t_c < MAP_COLS && t_r < MAP_ROWS {
            match self.grid.tiles[t_r * MAP_COLS + t_c] {
                TileType::GoalBlue => { self.score_red += 1; self.trigger_goal(); }
                TileType::GoalRed => { self.score_blue += 1; self.trigger_goal(); }
                _ => {}
            }
        }
    }

    fn trigger_goal(&mut self) {
        self.ball_carrier = usize::MAX;
        if self.score_blue >= 2 || self.score_red >= 2 {
            self.phase = 2; // Match Over
        } else {
            self.phase = 1; // Goal Celebration Pause
            self.timer = 2.5;
        }
    }

    fn reset_pitch(&mut self) {
        self.ball_pos = Vec2::new((MAP_COLS as f32 * TILE_SIZE) * 0.5, (MAP_ROWS as f32 * TILE_SIZE) * 0.5);
        self.ball_vel = Vec2::ZERO;
        self.ball_carrier = usize::MAX;
        self.kick_cooldown = 0.5;
        self.projs.count = 0; // Clear all active projectiles

        for p in self.players.iter_mut() {
            p.hp = p.max_hp; p.ammo = 3.0; p.respawn = 0.0;
            p.pos = Self::get_spawn(p.team);
        }
    }

    fn get_spawn(team: u8) -> Vec2 {
        if team == TEAM_BLUE { Vec2::new(150.0, 544.0) } else { Vec2::new(550.0, 544.0) } // Adjust to actual map center
    }

    pub fn apply_input(&mut self, p_idx: usize, input: PlayerInput) {
        if p_idx >= self.players.len() || self.phase != 0 { return; }
        let p = &mut self.players[p_idx];
        if p.respawn > 0.0 || p.is_airborne || p.action.is_some() { return; }

        let m = Vec2::new(input.move_x, input.move_y);
        p.vel = if m.length_squared() > 0.01 { m.normalize() * p.speed } else { Vec2::ZERO };
        
        let aim = Vec2::new(input.aim_x, input.aim_y);
        if aim.length_squared() > 0.01 {
            let n = aim.normalize(); p.rot = n.y.atan2(n.x);
        }

        let dir = Vec2::new(p.rot.cos(), p.rot.sin());
        let has_ball = self.ball_carrier == p_idx;

        if input.attack == 2 {
            if has_ball && p.super_charge >= 100.0 {
                // SUPER KICK
                self.ball_carrier = usize::MAX;
                self.ball_vel = dir * 1400.0;
                self.kick_cooldown = 0.4;
                p.super_charge = 0.0; p.out_of_combat = 0.0;
            } else if !has_ball && p.super_charge >= 100.0 {
                p.super_charge = 0.0; p.out_of_combat = 0.0;
                match p.brawler {
                    BrawlerId::Shelly => { // Wall breaking shotgun blast
                        for i in 0..9 {
                            let d = Vec2::new((p.rot + (i as f32 - 4.0)*0.08).cos(), (p.rot + (i as f32 - 4.0)*0.08).sin());
                            self.projs.spawn(p.pos, d * 1100.0, 0.55, 480.0, 9.0, p_idx, ProjectileFlags::WALL_BREAKER | ProjectileFlags::PIERCING | ProjectileFlags::KNOCKBACK);
                        }
                    },
                    BrawlerId::Colt => { // 12 Piercing Bullet wave
                        for i in 0..12 {
                            let d = Vec2::new((p.rot + (i as f32 * 0.015 - 0.08)).cos(), (p.rot + (i as f32 * 0.015 - 0.08)).sin());
                            self.projs.spawn(p.pos, d * 1250.0, 0.7, 520.0, 8.0, p_idx, ProjectileFlags::WALL_BREAKER | ProjectileFlags::PIERCING);
                        }
                    },
                    BrawlerId::ElPrimo => { // Leap
                        p.action = Some(ActiveAction { action_type: 2, timer: 0.85, step: 0, dir });
                    },
                    BrawlerId::Mortis => { // Lifesteal bat swarm
                        self.projs.spawn(p.pos, dir * 1000.0, 0.8, 1250.0, 16.0, p_idx, ProjectileFlags::LIFESTEAL | ProjectileFlags::PIERCING);
                    },
                    BrawlerId::Spike => { // Slowing Patch (Simulated as lingering hitbox here)
                        self.projs.spawn(p.pos + dir * 200.0, Vec2::ZERO, 4.5, 600.0, 65.0, p_idx, ProjectileFlags::PIERCING);
                    }
                }
            }
        } else if input.attack == 1 {
            if has_ball && p.ammo >= 1.0 {
                // NORMAL PASS
                self.ball_carrier = usize::MAX;
                self.ball_vel = dir * 850.0;
                self.kick_cooldown = 0.3;
                p.ammo -= 1.0; p.out_of_combat = 0.0;
            } else if !has_ball && p.ammo >= 1.0 {
                p.ammo -= 1.0; p.out_of_combat = 0.0;
                match p.brawler {
                    BrawlerId::Shelly => {
                        for &offset in &[-0.26, -0.13, 0.0, 0.13, 0.26] {
                            let d = Vec2::new((p.rot + offset).cos(), (p.rot + offset).sin());
                            self.projs.spawn(p.pos, d * 850.0, 0.45, 420.0, 7.5, p_idx, ProjectileFlags::NORMAL);
                        }
                    },
                    BrawlerId::Colt => { p.action = Some(ActiveAction { action_type: 1, timer: 0.0, step: 0, dir }); },
                    BrawlerId::ElPrimo => {
                        for i in 1..=4 { self.projs.spawn(p.pos + dir * (i as f32 * 8.0), dir * 700.0, 0.18, 540.0, 18.0, p_idx, ProjectileFlags::NORMAL); }
                    },
                    BrawlerId::Mortis => { p.action = Some(ActiveAction { action_type: 3, timer: 0.2, step: 0, dir }); },
                    BrawlerId::Spike => {
                        self.projs.spawn(p.pos, dir * 720.0, 0.52, 800.0, 10.0, p_idx, ProjectileFlags::EXPLODE);
                    }
                }
            }
        }
    }
}

// =====================================================================
// 5. C FFI / WASM EXPORTS (RAW POINTERS)
// =====================================================================

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_create() -> *mut Engine {
    let mut e = Box::new(Engine::new());
    
    // Auto-setup 3v3 Match
    e.spawn_player(1, TEAM_BLUE, BrawlerId::Shelly, Vec2::new(100.0, 400.0));
    e.spawn_player(2, TEAM_BLUE, BrawlerId::Colt, Vec2::new(100.0, 544.0));
    e.spawn_player(3, TEAM_BLUE, BrawlerId::ElPrimo, Vec2::new(100.0, 688.0));

    e.spawn_player(4, TEAM_RED, BrawlerId::Mortis, Vec2::new(600.0, 400.0));
    e.spawn_player(5, TEAM_RED, BrawlerId::Spike, Vec2::new(600.0, 544.0));
    e.spawn_player(6, TEAM_RED, BrawlerId::Shelly, Vec2::new(600.0, 688.0));

    Box::into_raw(e)
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_tick(engine: *mut Engine, dt: f32) {
    let e = &mut *engine;
    e.tick(dt);
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_apply_input(engine: *mut Engine, player_idx: u32, input: PlayerInput) {
    let e = &mut *engine;
    e.apply_input(player_idx as usize, input);
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_get_players_ptr(engine: *mut Engine) -> *const PlayerSnapshot {
    let e = &*engine;
    e.snapshots.as_ptr()
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_get_ball_ptr(engine: *mut Engine) -> *const BallSnapshot {
    let e = &*engine;
    &e.ball_snap
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_get_match_ptr(engine: *mut Engine) -> *const MatchSnapshot {
    let e = &*engine;
    &e.match_snap
}

#[ffi_function]
#[no_mangle]
pub unsafe extern "C" fn engine_destroy(engine: *mut Engine) {
    if !engine.is_null() {
        let _ = Box::from_raw(engine); // Drops and frees memory
    }
}

// =====================================================================
// 6. INTEROPTOPUS INVENTORY BUILDER
// =====================================================================

pub fn my_inventory() -> Inventory {
    InventoryBuilder::new()
        .register(function!(_export_models))
        .register(function!(engine_create))
        .register(function!(engine_tick))
        .register(function!(engine_apply_input))
        .register(function!(engine_get_players_ptr))
        .register(function!(engine_get_ball_ptr))
        .register(function!(engine_get_match_ptr))
        .register(function!(engine_destroy))
        .inventory()
}
