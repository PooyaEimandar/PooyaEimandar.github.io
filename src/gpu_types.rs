//! GPU data layouts only. Keep runtime behavior outside this module so its
//! exemption for derive-generated compile-time assertions stays narrowly scoped.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct MatrixUniforms {
    pub(super) timing: [f32; 4],
    pub(super) viewport: [f32; 4],
    pub(super) signal: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct PortraitUniforms {
    pub(super) timing: [f32; 4],
    pub(super) viewport: [f32; 4],
    pub(super) placement: [f32; 4],
    pub(super) eyes: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct TextUniforms {
    pub(super) view_projection: [[f32; 4]; 4],
    pub(super) model: [[f32; 4]; 4],
    pub(super) params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub(super) struct TerminalVertex {
    pub(super) position: [f32; 3],
    pub(super) color: [f32; 4],
    /// Character reveal order and vertex kind (0 = glyph, 1 = cursor).
    pub(super) meta: [f32; 2],
}
