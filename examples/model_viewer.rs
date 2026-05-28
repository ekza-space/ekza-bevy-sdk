use std::{env, fs, path::PathBuf, process::ExitCode};

use bevy::{
    asset::AssetPlugin,
    camera::primitives::Aabb,
    input::mouse::{MouseMotion, MouseWheel},
    prelude::*,
    scene::SceneRoot,
};
use ekza_bevy_sdk::{
    EkzaCharacter, GlbValidationRules,
    bevy::{EkzaModelHandles, load_model_entry},
    builtin_model_entry, validate_glb_file,
};

const FRAME_TARGET_MAX_EXTENT: f32 = 2.0;
const FRAME_MIN_EXTENT: f32 = 0.001;
const FRAME_WARN_AFTER_FRAMES: u32 = 240;

#[derive(Resource)]
struct ViewerConfig {
    asset_root: PathBuf,
    source: ViewerSource,
    scene_label: String,
}

enum ViewerSource {
    Character(EkzaCharacter),
    Glb { relative_path: String },
}

#[derive(Component)]
struct OrbitCamera {
    focus: Vec3,
    radius: f32,
    yaw: f32,
    pitch: f32,
}

#[derive(Component)]
struct ViewedModelRoot {
    label: String,
    framed: bool,
    waited_frames: u32,
}

fn main() -> ExitCode {
    let config = match parse_config() {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            print_usage();
            return ExitCode::from(64);
        }
    };

    println!("Ekza model viewer");
    println!("asset root: {}", config.asset_root.display());
    println!("controls: hold right mouse and drag to orbit, mouse wheel to zoom, Esc to quit");

    let asset_root = config.asset_root.to_string_lossy().to_string();
    App::new()
        .insert_resource(ClearColor(Color::srgb(0.035, 0.04, 0.05)))
        .insert_resource(config)
        .add_plugins(DefaultPlugins.set(AssetPlugin {
            file_path: asset_root,
            ..default()
        }))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (frame_loaded_model, orbit_camera_input, exit_on_escape),
        )
        .run();
    ExitCode::SUCCESS
}

fn parse_config() -> Result<ViewerConfig, String> {
    let mut args = env::args().skip(1);
    let mut asset_root = default_asset_root();
    let mut character = EkzaCharacter::Ipfs;
    let mut explicit_glb: Option<PathBuf> = None;
    let mut scene_label = "Scene0".to_string();

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--asset-root" => {
                let Some(raw) = args.next() else {
                    return Err("--asset-root requires a path".to_string());
                };
                asset_root = PathBuf::from(raw);
            }
            "--character" => {
                let Some(raw) = args.next() else {
                    return Err("--character requires ipfs, toka, wang, or cube".to_string());
                };
                character = parse_character(&raw)?;
            }
            "--glb" => {
                let Some(raw) = args.next() else {
                    return Err("--glb requires a path".to_string());
                };
                explicit_glb = Some(PathBuf::from(raw));
            }
            "--scene-label" => {
                let Some(raw) = args.next() else {
                    return Err("--scene-label requires a glTF scene label".to_string());
                };
                scene_label = raw;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }

    let source = if let Some(glb) = explicit_glb {
        let (resolved_root, relative_path) = prepare_explicit_glb(&asset_root, glb)?;
        asset_root = resolved_root;
        ViewerSource::Glb { relative_path }
    } else {
        asset_root = existing_absolute_path(asset_root);
        ViewerSource::Character(character)
    };

    Ok(ViewerConfig {
        asset_root,
        source,
        scene_label,
    })
}

fn default_asset_root() -> PathBuf {
    PathBuf::from("assets")
}

fn parse_character(raw: &str) -> Result<EkzaCharacter, String> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "ipfs" => Ok(EkzaCharacter::Ipfs),
        "toka" => Ok(EkzaCharacter::Toka),
        "wang" => Ok(EkzaCharacter::Wang),
        "cube" => Ok(EkzaCharacter::Cube),
        _ => Err(format!("unknown character: {raw}")),
    }
}

fn prepare_explicit_glb(asset_root: &PathBuf, glb: PathBuf) -> Result<(PathBuf, String), String> {
    if glb.is_relative() && asset_root.join(&glb).exists() {
        validate_file_or_error(&asset_root.join(&glb))?;
        return Ok((
            existing_absolute_path(asset_root.clone()),
            glb.to_string_lossy().to_string(),
        ));
    }

    if !glb.exists() {
        return Err(format!("GLB file does not exist: {}", glb.display()));
    }
    validate_file_or_error(&glb)?;

    let file_name = glb
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model.glb");
    let cache_root = env::current_dir()
        .map_err(|error| format!("failed to resolve current directory: {error}"))?
        .join("target/ekza-bevy-sdk-viewer-assets");
    let relative_path = format!("imported/{file_name}");
    let final_path = cache_root.join(&relative_path);
    if let Some(parent) = final_path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create viewer cache {parent:?}: {error}"))?;
    }
    fs::copy(&glb, &final_path).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            glb.display(),
            final_path.display()
        )
    })?;

    Ok((existing_absolute_path(cache_root), relative_path))
}

fn existing_absolute_path(path: PathBuf) -> PathBuf {
    path.canonicalize().unwrap_or(path)
}

fn validate_file_or_error(path: &PathBuf) -> Result<(), String> {
    let report = validate_glb_file(path, &GlbValidationRules::default())
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    if report.is_valid() {
        Ok(())
    } else {
        let issues = report
            .issues()
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("; ");
        Err(format!(
            "{} failed GLB validation: {issues}",
            path.display()
        ))
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    config: Res<ViewerConfig>,
) {
    let handles = match &config.source {
        ViewerSource::Character(character) => {
            let entry = *builtin_model_entry(*character);
            load_model_entry(entry, &asset_server, &config.asset_root)
        }
        ViewerSource::Glb { relative_path } => EkzaModelHandles {
            scene: Some(asset_server.load(format!("{relative_path}#{}", config.scene_label))),
            gltf: Some(asset_server.load(relative_path.clone())),
            label: relative_path.clone(),
        },
    };

    if let Some(scene) = handles.scene {
        commands.spawn((
            SceneRoot(scene),
            Transform::from_translation(Vec3::ZERO),
            ViewedModelRoot {
                label: handles.label.clone(),
                framed: false,
                waited_frames: 0,
            },
            Name::new(format!("EkzaModel-{}", handles.label)),
        ));
    } else {
        let mesh = meshes.add(Cuboid::new(1.0, 1.0, 1.0));
        let material = materials.add(StandardMaterial::from(Color::srgb(0.8, 0.7, 0.6)));
        commands.spawn((
            Mesh3d(mesh),
            MeshMaterial3d(material),
            Transform::from_translation(Vec3::new(0.0, 0.5, 0.0)),
            ViewedModelRoot {
                label: "PrimitiveFallback".to_string(),
                framed: false,
                waited_frames: 0,
            },
            Name::new("PrimitiveFallback"),
        ));
    }

    let floor_mesh = meshes.add(Cuboid::new(8.0, 0.02, 8.0));
    let floor_material = materials.add(StandardMaterial::from(Color::srgb(0.18, 0.2, 0.22)));
    commands.spawn((
        Mesh3d(floor_mesh),
        MeshMaterial3d(floor_material),
        Transform::from_translation(Vec3::new(0.0, -0.02, 0.0)),
        Name::new("ViewerFloor"),
    ));

    commands.spawn((
        DirectionalLight {
            illuminance: 25_000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::ZYX, 0.0, 0.75, -0.8)),
        Name::new("ViewerKeyLight"),
    ));
    commands.insert_resource(GlobalAmbientLight {
        brightness: 450.0,
        ..default()
    });

    let camera = OrbitCamera {
        focus: Vec3::new(0.0, 0.6, 0.0),
        radius: 3.0,
        yaw: -0.75,
        pitch: -0.35,
    };
    commands.spawn((
        Camera3d::default(),
        orbit_transform(&camera),
        camera,
        Name::new("ViewerCamera"),
    ));
}

fn frame_loaded_model(
    mut model_query: Query<(Entity, &mut Transform, &mut ViewedModelRoot), Without<OrbitCamera>>,
    children_query: Query<&Children>,
    aabb_query: Query<&Aabb>,
    global_query: Query<&GlobalTransform>,
    mut camera_query: Query<(&mut Transform, &mut OrbitCamera), Without<ViewedModelRoot>>,
) {
    let Ok((entity, mut transform, mut model)) = model_query.single_mut() else {
        return;
    };
    if model.framed {
        return;
    }

    let Some((min, max)) = model_bounds(entity, &children_query, &aabb_query, &global_query) else {
        model.waited_frames += 1;
        if model.waited_frames == FRAME_WARN_AFTER_FRAMES {
            warn!(
                "No render bounds found for {} yet. The GLB may have no mesh scene, an unexpected scene label, or assets still loading.",
                model.label
            );
        }
        return;
    };

    let size = max - min;
    let max_extent = size.max_element();
    if max_extent <= FRAME_MIN_EXTENT {
        warn!(
            "Model {} has near-zero bounds: min={min:?} max={max:?}",
            model.label
        );
        model.framed = true;
        return;
    }

    let scale = FRAME_TARGET_MAX_EXTENT / max_extent;
    let center = (min + max) * 0.5;
    transform.scale = Vec3::splat(scale);
    transform.translation = Vec3::new(-center.x * scale, -min.y * scale, -center.z * scale);
    model.framed = true;

    let framed_height = size.y * scale;
    let focus = Vec3::new(0.0, framed_height.max(0.4) * 0.5, 0.0);
    let radius = (FRAME_TARGET_MAX_EXTENT * 1.8).max(2.4);
    if let Ok((mut camera_transform, mut orbit)) = camera_query.single_mut() {
        orbit.focus = focus;
        orbit.radius = radius;
        *camera_transform = orbit_transform(&orbit);
    }

    info!(
        "Framed model {}: min={min:?} max={max:?} scale={scale:.4} focus={focus:?}",
        model.label
    );
    println!(
        "Framed model {}: size={:.3} x {:.3} x {:.3}, scale={scale:.4}",
        model.label, size.x, size.y, size.z
    );
}

fn model_bounds(
    root: Entity,
    children_query: &Query<&Children>,
    aabb_query: &Query<&Aabb>,
    global_query: &Query<&GlobalTransform>,
) -> Option<(Vec3, Vec3)> {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    let mut found = false;

    for entity in std::iter::once(root).chain(children_query.iter_descendants(root)) {
        let (Ok(aabb), Ok(global)) = (aabb_query.get(entity), global_query.get(entity)) else {
            continue;
        };
        let center: Vec3 = aabb.center.into();
        let half: Vec3 = aabb.half_extents.into();

        for sx in [-1.0_f32, 1.0] {
            for sy in [-1.0_f32, 1.0] {
                for sz in [-1.0_f32, 1.0] {
                    let local_corner = center + Vec3::new(half.x * sx, half.y * sy, half.z * sz);
                    let world_corner = global.transform_point(local_corner);
                    min = min.min(world_corner);
                    max = max.max(world_corner);
                    found = true;
                }
            }
        }
    }

    found.then_some((min, max))
}

fn orbit_camera_input(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut motion_events: MessageReader<MouseMotion>,
    mut wheel_events: MessageReader<MouseWheel>,
    mut camera_query: Query<(&mut Transform, &mut OrbitCamera)>,
) {
    let Ok((mut transform, mut orbit)) = camera_query.single_mut() else {
        return;
    };

    if mouse_buttons.pressed(MouseButton::Right) {
        for event in motion_events.read() {
            orbit.yaw -= event.delta.x * 0.006;
            orbit.pitch = (orbit.pitch - event.delta.y * 0.006).clamp(-1.3, 1.2);
        }
    } else {
        motion_events.clear();
    }

    for event in wheel_events.read() {
        orbit.radius = (orbit.radius - event.y * 0.18).clamp(0.8, 12.0);
    }

    *transform = orbit_transform(&orbit);
}

fn orbit_transform(orbit: &OrbitCamera) -> Transform {
    let rotation = Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.0);
    let offset = rotation * Vec3::new(0.0, 0.0, orbit.radius);
    Transform::from_translation(orbit.focus + offset).looking_at(orbit.focus, Vec3::Y)
}

fn exit_on_escape(keys: Res<ButtonInput<KeyCode>>, mut exit: MessageWriter<AppExit>) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
}

fn print_usage() {
    eprintln!(
        "Usage:
  cargo run --example model_viewer -- [--character ipfs|toka|wang|cube] [--asset-root PATH]
  cargo run --example model_viewer -- --glb path/to/model.glb [--scene-label Scene0]"
    );
}
