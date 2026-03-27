#import bevy_pbr::{
    mesh_functions,
    skinning,
    forward_io::{Vertex, VertexOutput},
    mesh_view_bindings::view,
}

@vertex
fn vertex(vertex_no_morph: Vertex) -> VertexOutput {
    var out: VertexOutput;
    var vertex = vertex_no_morph;

#ifdef SKINNED
    var world_from_local = skinning::skin_model(
        vertex.joint_indices,
        vertex.joint_weights,
        vertex_no_morph.instance_index,
    );
#else
    var world_from_local = mesh_functions::get_world_from_local(
        vertex_no_morph.instance_index,
    );
#endif

    // Spherical billboard with 2D rotation preservation.
    //
    // Strips all rotation from model-view (making quads face the screen),
    // then re-applies the bone's accumulated Z-rotation as a screen-plane
    // tilt. For entities with no Z-rotation (health bars), this reduces
    // to a plain spherical billboard.

    var model_view = view.view_from_world * world_from_local;

    let scale_x = length(model_view[0].xyz);
    let scale_y = length(model_view[1].xyz);
    let scale_z = length(model_view[2].xyz);

    let det = determinant(mat3x3f(
        model_view[0].xyz,
        model_view[1].xyz,
        model_view[2].xyz,
    ));
    let flip = select(1.0, -1.0, det < 0.0);

    // Extract bone Z-rotation from world_from_local column 0.
    // For pure Z-rotation rigs: column 0 = (flip * cos θ * sx, sin θ * sx, 0).
    // When non-Z rotations exist (e.g. character Y-rotation on health bars),
    // the XY components shrink. Normalizing ensures a unit-length rotation
    // so non-Z rotations don't scale the billboard.
    let raw_cos = world_from_local[0].x / flip;
    let raw_sin = world_from_local[0].y;
    let rot_len = sqrt(raw_cos * raw_cos + raw_sin * raw_sin);
    let sin_t = select(0.0, raw_sin / rot_len, rot_len > 0.0001);
    let cos_t = select(1.0, raw_cos / rot_len, rot_len > 0.0001);

    // Spherical billboard + normalized screen-plane bone rotation.
    model_view[0] = vec4<f32>(flip * scale_x * cos_t, flip * scale_x * sin_t, 0.0, model_view[0][3]);
    model_view[1] = vec4<f32>(-scale_y * sin_t, scale_y * cos_t, 0.0, model_view[1][3]);
    model_view[2] = vec4<f32>(0.0, 0.0, scale_z, model_view[2][3]);

    let view_pos = model_view * vec4<f32>(vertex.position, 1.0);
    out.position = view.clip_from_view * view_pos;

    let world_pos = world_from_local * vec4<f32>(vertex.position, 1.0);
    out.world_position = world_pos;

#ifdef VERTEX_NORMALS
    out.world_normal = normalize(
        (view.world_from_view * vec4<f32>(0.0, 0.0, 1.0, 0.0)).xyz
    );
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex_no_morph.instance_index;
#endif

    return out;
}
