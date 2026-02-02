use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface, LineCap, SvgSurface};
use clap::{Parser, Subcommand};
use pango::{Alignment, FontDescription};
use pangocairo::functions as pangocairo;
use roxmltree::Document;

const DEFAULT_PADDING_PX: f64 = 10.0;
const DEFAULT_LINE_WIDTH: f64 = 1.5;
const FONT_MAIN_PX: f64 = 20.0;
const FONT_SMALL_PX: f64 = 12.0;
const FONT_FAMILY: &str = "Liberation Sans";
const TEXT_OUTLINE_WIDTH: f64 = 0.75;
const ARROW_SIZE: f64 = 8.0;
const ARROW_SCALE: f64 = 1.75;
const BAR_LENGTH: f64 = 12.0;
const BAR_OFFSET: f64 = 14.0;
const CATALYSIS_OVERLAP_RATIO: f64 = 0.5;
const PORT_CONNECTOR_LEN_PX: f64 = 11.0;
const LOGICAL_PORT_CONNECTOR_LEN_PX: f64 = 20.0;
const SHOW_PROCESS_DEBUG: bool = false;
const SHOW_LOGICAL_DEBUG_BBOX: bool = false;
const BORDER_COLOR: (f64, f64, f64) = (0x55 as f64 / 255.0, 0x55 as f64 / 255.0, 0x55 as f64 / 255.0);
const DEFAULT_FILL_COLOR: (f64, f64, f64) = (0xF6 as f64 / 255.0, 0xF6 as f64 / 255.0, 0xF6 as f64 / 255.0);
const AUX_LINE_COLOR: (f64, f64, f64) = (0x6A as f64 / 255.0, 0x6A as f64 / 255.0, 0x6A as f64 / 255.0);
const ASSOCIATION_FILL_COLOR: (f64, f64, f64) = (0x6B as f64 / 255.0, 0x6B as f64 / 255.0, 0x6B as f64 / 255.0);
const CLONE_MARKER_HEIGHT_RATIO: f64 = 0.30;
const CLONE_MARKER_FILL_COLOR: (f64, f64, f64) = (0.82, 0.82, 0.82);
const CLONE_MARKER_STROKE_WIDTH: f64 = 1.5;

#[derive(Parser)]
#[command(author, version, about = "Render SBGNML diagrams to PNG", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(name = "draw_sbgnml")]
    DrawSbgnml {
        #[arg(long)]
        input: PathBuf,
        #[arg(long, default_value = "sbgnml.png")]
        output: PathBuf,
        #[arg(long, default_value_t = DEFAULT_PADDING_PX)]
        padding: f64,
        #[arg(long, default_value_t = true)]
        clone_markers: bool,
    },
}

#[derive(Clone, Copy, Debug)]
struct Point {
    x: f64,
    y: f64,
}

#[derive(Clone, Copy, Debug)]
struct BBox {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

#[derive(Clone, Copy, Debug)]
struct PixelRect {
    x0: f64,
    y0: f64,
    width: f64,
    height: f64,
    center: Point,
}

#[derive(Debug)]
struct Glyph {
    id: String,
    parent_id: Option<String>,
    class_name: String,
    bbox: Option<BBox>,
    label: String,
    ports: Vec<Point>,
    has_clone: bool,
    state_value: Option<String>,
    state_variable: Option<String>,
    orientation: Option<String>,
}

#[derive(Debug)]
struct Arc {
    class_name: String,
    points: Vec<Point>,
}

#[derive(Clone, Copy, Debug)]
struct Bounds {
    min_x: f64,
    max_x: f64,
    min_y: f64,
    max_y: f64,
}

#[derive(Clone, Copy, Debug)]
struct Transform {
    min_x: f64,
    min_y: f64,
    scale_x: f64,
    scale_y: f64,
}

impl Transform {
    fn new(min_x: f64, min_y: f64, max_x: f64, max_y: f64, width: f64, height: f64) -> Self {
        let span_x = (max_x - min_x).abs().max(1.0);
        let span_y = (max_y - min_y).abs().max(1.0);
        let scale_x = width / span_x;
        let scale_y = height / span_y;
        Self {
            min_x,
            min_y,
            scale_x,
            scale_y,
        }
    }

    fn map_point(&self, x: f64, y: f64) -> Point {
        Point {
            x: (x - self.min_x) * self.scale_x,
            y: (y - self.min_y) * self.scale_y,
        }
    }

    fn map_size(&self, w: f64, h: f64) -> (f64, f64) {
        (w * self.scale_x, h * self.scale_y)
    }

    fn scale_scalar(&self, value: f64) -> f64 {
        value * self.scale_x.min(self.scale_y)
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::DrawSbgnml {
            input,
            output,
            padding,
            clone_markers,
        } => {
            let svg_path = default_svg_output_path(&output);
            draw_sbgnml(&input, &output, padding, &svg_path, clone_markers)
        }
    }
}

fn setup_context(ctx: &CairoContext) -> Result<()> {
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.paint()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    ctx.set_line_cap(LineCap::Square);
    Ok(())
}

fn create_png_surface(width: i32, height: i32) -> Result<(ImageSurface, CairoContext)> {
    let surface = ImageSurface::create(Format::ARgb32, width, height)
        .context("Failed to create image surface")?;
    let ctx = CairoContext::new(&surface).context("Failed to create Cairo context")?;
    setup_context(&ctx)?;
    Ok((surface, ctx))
}

fn default_svg_output_path(output: &Path) -> PathBuf {
    let mut svg_path = output.to_path_buf();
    svg_path.set_extension("svg");
    svg_path
}

fn render_svg<F>(svg_path: &Path, width: f64, height: f64, render: F) -> Result<()>
where
    F: FnOnce(&CairoContext) -> Result<()>,
{
    let surface = SvgSurface::new(width, height, Some(svg_path))
        .context("Failed to create SVG surface")?;
    let ctx = CairoContext::new(&surface).context("Failed to create Cairo context")?;
    setup_context(&ctx)?;
    render(&ctx)?;
    surface.finish();
    Ok(())
}

fn draw_sbgnml(
    input: &Path,
    output: &Path,
    padding: f64,
    svg_output: &Path,
    show_clone_markers: bool,
) -> Result<()> {
    let xml = fs::read_to_string(input).with_context(|| format!("Failed to read {:?}", input))?;
    let doc = Document::parse(&xml).context("Failed to parse SBGN XML")?;
    let (glyphs, arcs, bounds) = parse_sbgn(&doc)?;

    let (transform, width_f, height_f) = transform_with_padding(bounds, padding);
    let (surface, ctx) = create_png_surface(width_f.ceil() as i32, height_f.ceil() as i32)?;
    render_sbgnml(&ctx, &transform, &glyphs, &arcs, show_clone_markers)?;

    let mut file = fs::File::create(output).context("Failed to create PNG file")?;
    surface
        .write_to_png(&mut file)
        .context("Failed to write PNG")?;

    render_svg(svg_output, width_f, height_f, |ctx| {
        render_sbgnml(ctx, &transform, &glyphs, &arcs, show_clone_markers)
    })?;
    Ok(())
}

/// Render parsed SBGNML glyphs and arcs using bbox geometry.
fn render_sbgnml(
    ctx: &CairoContext,
    transform: &Transform,
    glyphs: &[Glyph],
    arcs: &[Arc],
    show_clone_markers: bool,
) -> Result<()> {
    let mut child_map: HashMap<String, Vec<&Glyph>> = HashMap::new();
    for glyph in glyphs {
        if let Some(parent_id) = &glyph.parent_id {
            child_map
                .entry(parent_id.clone())
                .or_default()
                .push(glyph);
        }
    }

    let aux_glyphs: Vec<&Glyph> = glyphs
        .iter()
        .filter(|glyph| {
            glyph.parent_id.is_some()
                && matches!(
                    glyph.class_name.as_str(),
                    "unit of information" | "state variable"
                )
        })
        .collect();

    for glyph in glyphs.iter().filter(|glyph| glyph.parent_id.is_none()) {
        render_glyph_tree(ctx, transform, glyph, &child_map, show_clone_markers)?;
    }

    // Render auxiliary glyphs at their absolute bbox positions.
    for glyph in aux_glyphs {
        let bbox = match glyph.bbox {
            Some(bbox) => bbox,
            None => continue,
        };
        let class_name = glyph.class_name.as_str();
        let label = if class_name == "state variable" && glyph.label.trim().is_empty() {
            state_var_label(glyph.state_value.as_deref(), glyph.state_variable.as_deref())
        } else {
            glyph.label.clone()
        };
        let font_px = glyph_font_px(class_name);
        let has_clone = show_clone_markers && glyph.has_clone;
        match class_name {
            "unit of information" => {
                draw_round_rect_bbox(ctx, transform, bbox, &label, font_px, has_clone)?
            }
            "state variable" => {
                draw_stadium_bbox(ctx, transform, bbox, &label, font_px, has_clone)?
            }
            _ => {}
        }
    }

    let arrow_size_px = transform.scale_scalar(ARROW_SIZE * ARROW_SCALE);
    let bar_length_px = transform.scale_scalar(BAR_LENGTH * ARROW_SCALE);
    let bar_offset_px = transform.scale_scalar(BAR_OFFSET * ARROW_SCALE);

    for arc in arcs {
        let points_px: Vec<Point> = arc
            .points
            .iter()
            .map(|pt| transform.map_point(pt.x, pt.y))
            .collect();
        draw_arc(ctx, &points_px, &arc.class_name, arrow_size_px, bar_length_px, bar_offset_px)?;
    }
    Ok(())
}

fn render_glyph_tree(
    ctx: &CairoContext,
    transform: &Transform,
    glyph: &Glyph,
    child_map: &HashMap<String, Vec<&Glyph>>,
    show_clone_markers: bool,
) -> Result<()> {
    let bbox = match glyph.bbox {
        Some(bbox) => bbox,
        None => return Ok(()),
    };

    let class_name = glyph.class_name.as_str();
    let class_base = class_name.strip_suffix(" multimer").unwrap_or(class_name);
    let is_multimer = class_name.ends_with(" multimer");
    let label_override = match class_name {
        "and" => Some("AND"),
        "or" => Some("OR"),
        "not" => Some("NOT"),
        "omitted process" => Some("\\\\"),
        "uncertain process" => Some("?"),
        _ => None,
    };
    let mut label = label_override.unwrap_or(glyph.label.as_str()).to_string();
    if class_name == "state variable" && label.trim().is_empty() {
        label = state_var_label(glyph.state_value.as_deref(), glyph.state_variable.as_deref());
    }
    let font_px = glyph_font_px(class_name);
    let has_clone = show_clone_markers && glyph.has_clone;
    let children = child_map.get(&glyph.id).map(|items| items.as_slice()).unwrap_or(&[]);
    let has_u_info_bbox = children.iter().any(|child| {
        child.class_name == "unit of information" && child.bbox.is_some()
    });
    let has_s_var_bbox = children.iter().any(|child| {
        child.class_name == "state variable" && child.bbox.is_some()
    });
    let u_info_label = if has_u_info_bbox {
        None
    } else {
        first_child_label(children, "unit of information")
    };
    let s_var_label = if has_s_var_bbox {
        None
    } else {
        first_child_state_label(children, "state variable")
    };
    let place_label_bottom = class_base == "complex" || class_name == "compartment";
    let shape_label = if place_label_bottom { "" } else { label.as_str() };

    match class_name {
        "phenotype" | "outcome" => {
            draw_hexagon_bbox(ctx, transform, bbox, shape_label, font_px, false)?
        }
        "perturbing agent" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                None,
            )?;
        }
        "simple chemical" | "simple chemical multimer" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                None,
            )?;
        }
        "unspecified entity" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                s_var_label.as_deref(),
            )?;
        }
        "macromolecule" | "macromolecule multimer" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                s_var_label.as_deref(),
            )?;
        }
        "nucleic acid feature" | "nucleic acid feature multimer" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                s_var_label.as_deref(),
            )?;
        }
        "complex" | "complex multimer" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                s_var_label.as_deref(),
            )?;
        }
        "source and sink" => draw_source_sink_bbox(ctx, transform, bbox, has_clone)?,
        "compartment" => draw_barrel_bbox(ctx, transform, bbox, shape_label, font_px, has_clone)?,
        "tag" => draw_tag_bbox(ctx, transform, bbox, shape_label, font_px, has_clone)?,
        "association" => draw_ellipse_bbox_filled(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            ASSOCIATION_FILL_COLOR,
        )?,
        "dissociation" => draw_double_circle_bbox(ctx, transform, bbox, shape_label, font_px)?,
        "process" | "omitted process" | "uncertain process" => {
            draw_square_bbox(ctx, transform, bbox, shape_label, font_px, false)?;
            if SHOW_PROCESS_DEBUG {
                draw_process_debug_bbox(ctx, transform, bbox)?;
            }
        }
        "unit of information" => {
            draw_round_rect_bbox(ctx, transform, bbox, shape_label, font_px, false)?
        }
        "state variable" => {
            draw_stadium_bbox(ctx, transform, bbox, shape_label, font_px, false)?
        }
        "and" | "or" | "not" => {
            draw_circle_bbox(ctx, transform, bbox, shape_label, font_px)?;
            if SHOW_LOGICAL_DEBUG_BBOX {
                draw_logical_debug_bbox(ctx, transform, bbox)?;
            }
        }
        _ => draw_box_bbox(ctx, transform, bbox, shape_label, font_px, false)?,
    }

    let orientation = glyph.orientation.as_deref().or_else(|| {
        if matches!(
            class_name,
            "process" | "omitted process" | "uncertain process" | "association" | "dissociation"
        ) {
            Some("horizontal")
        } else {
            None
        }
    });
    if let Some(orientation) = orientation {
        let connector_len_px = port_connector_len_px_for_class(class_name);
        draw_orientation_marker(ctx, transform, bbox, orientation, connector_len_px)?;
    }

    if place_label_bottom {
        let rect = bbox_pixel_rect(transform, bbox);
        draw_text_bottom_centered(ctx, rect, &label, font_px)?;
    }

    for child in children.iter().copied() {
        if matches!(
            child.class_name.as_str(),
            "unit of information" | "state variable"
        ) {
            continue;
        }
        render_glyph_tree(ctx, transform, child, child_map, show_clone_markers)?;
    }

    Ok(())
}

fn draw_box_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        DEFAULT_LINE_WIDTH,
        Some(DEFAULT_FILL_COLOR),
        path_rect,
    )
}

fn draw_process_debug_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let inset_rect = PixelRect {
        x0: rect.x0 - 10.0,
        y0: rect.y0 - 10.0,
        width: rect.width + 20.0,
        height: rect.height + 20.0,
        center: rect.center,
    };
    ctx.set_source_rgb(1.0, 0.0, 1.0);
    ctx.set_line_width(1.0);
    path_rect(ctx, inset_rect)?;
    ctx.stroke()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn draw_logical_debug_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    ctx.set_source_rgb(1.0, 0.0, 1.0);
    ctx.set_line_width(1.0);
    path_rect(ctx, rect)?;
    ctx.stroke()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn draw_square_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let center_data = Point {
        x: bbox.x + bbox.w / 2.0,
        y: bbox.y + bbox.h / 2.0,
    };
    let side = bbox.w.min(bbox.h);
    let center = transform.map_point(center_data.x, center_data.y);
    let (side_px, _) = transform.map_size(side, side);
    let rect = PixelRect {
        x0: center.x - side_px / 2.0,
        y0: center.y - side_px / 2.0,
        width: side_px,
        height: side_px,
        center,
    };
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        DEFAULT_LINE_WIDTH,
        Some(DEFAULT_FILL_COLOR),
        path_rect,
    )
}

/// Draw an ellipse glyph filled with a custom color.
fn draw_ellipse_bbox_filled(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    fill: (f64, f64, f64),
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    path_ellipse(ctx, rect)?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    ctx.set_source_rgb(fill.0, fill.1, fill.2);
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px)?;
    Ok(())
}

fn draw_double_circle_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let radius = (rect.width.min(rect.height) / 2.0).max(1.0);
    ctx.new_path();
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    ctx.arc(rect.center.x, rect.center.y, radius, 0.0, std::f64::consts::TAU);
    ctx.set_source_rgb(
        DEFAULT_FILL_COLOR.0,
        DEFAULT_FILL_COLOR.1,
        DEFAULT_FILL_COLOR.2,
    );
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    ctx.new_path();
    ctx.arc(
        rect.center.x,
        rect.center.y,
        (radius * 0.6).max(1.0),
        0.0,
        std::f64::consts::TAU,
    );
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px)?;
    Ok(())
}

fn draw_round_rect_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        DEFAULT_LINE_WIDTH,
        Some(DEFAULT_FILL_COLOR),
        |ctx, rect| {
            let radius = (rect.width.min(rect.height) * 0.1).max(1.0);
            path_round_rect(ctx, rect, radius)
        },
    )
}

fn draw_hexagon_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        DEFAULT_LINE_WIDTH,
        Some(DEFAULT_FILL_COLOR),
        path_hexagon,
    )
}

fn draw_source_sink_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    path_ellipse(ctx, rect)?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    ctx.set_source_rgb(
        DEFAULT_FILL_COLOR.0,
        DEFAULT_FILL_COLOR.1,
        DEFAULT_FILL_COLOR.2,
    );
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    if has_clone {
        draw_clone_marker(ctx, rect, &path_ellipse)?;
        path_ellipse(ctx, rect)?;
        ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
        ctx.stroke()?;
    }
    ctx.new_path();
    ctx.move_to(rect.x0, rect.y0 + rect.height);
    ctx.line_to(rect.x0 + rect.width, rect.y0);
    ctx.stroke()?;
    Ok(())
}

fn draw_barrel_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let border_width = 4.0;
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        border_width,
        Some(DEFAULT_FILL_COLOR),
        path_barrel,
    )
}

fn draw_tag_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        DEFAULT_LINE_WIDTH,
        Some(DEFAULT_FILL_COLOR),
        |ctx, rect| {
            let notch = (rect.height * 0.3).max(2.0);
            path_tag(ctx, rect, notch)
        },
    )
}

fn draw_stadium_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        has_clone,
        DEFAULT_LINE_WIDTH,
        Some(DEFAULT_FILL_COLOR),
        |ctx, rect| {
            let radius = 0.24 * rect.width.max(rect.height);
            path_round_rect_impl(ctx, rect.x0, rect.y0, rect.width, rect.height, radius)
        },
    )
}

/// Draw an entity pool node using shapes and auxiliary items from sbgnStyle.
fn draw_entity_pool_node(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    class_name: &str,
    is_multimer: bool,
    has_clone: bool,
    u_info_label: Option<&str>,
    s_var_label: Option<&str>,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let (ref_w, ref_h) = default_dimensions(class_name).unwrap_or((rect.width, rect.height));
    let scale_x = rect.width / ref_w;
    let scale_y = rect.height / ref_h;
    // Multimers are drawn as a "ghost" shape offset behind the main glyph.
    if is_multimer {
        if let Some((ghost_dx, ghost_dy)) = ghost_offset_for(class_name) {
            let ghost_rect = PixelRect {
                x0: rect.x0 + ghost_dx * scale_x,
                y0: rect.y0 + ghost_dy * scale_y,
                width: rect.width,
                height: rect.height,
                center: Point {
                    x: rect.center.x + ghost_dx * scale_x,
                    y: rect.center.y + ghost_dy * scale_y,
                },
            };
            draw_entity_pool_base_shape(
                ctx,
                ghost_rect,
                class_name,
                "",
                FONT_SMALL_PX,
                false,
                entity_pool_fill_color(class_name),
                entity_pool_border_width(class_name),
            )?;
        }
    }

    draw_entity_pool_base_shape(
        ctx,
        rect,
        class_name,
        label,
        font_px,
        has_clone,
        entity_pool_fill_color(class_name),
        entity_pool_border_width(class_name),
    )?;

    draw_entity_pool_aux_items(
        ctx,
        rect,
        class_name,
        u_info_label,
        s_var_label,
    )?;
    Ok(())
}

/// Draw the base shape for entity pool nodes without labels or overlays.
fn draw_entity_pool_base_shape(
    ctx: &CairoContext,
    rect: PixelRect,
    class_name: &str,
    label: &str,
    font_px: f64,
    has_clone: bool,
    fill_color: Option<(f64, f64, f64)>,
    border_width: f64,
) -> Result<()> {
    match class_name {
        "simple chemical" | "unspecified entity" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            has_clone,
            border_width,
            fill_color,
            path_ellipse,
        ),
        "macromolecule" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            has_clone,
            border_width,
            fill_color,
            |ctx, rect| {
                let radius = (rect.width.min(rect.height) * 0.1).max(1.0);
                path_round_rect_impl(ctx, rect.x0, rect.y0, rect.width, rect.height, radius)
            },
        ),
        "nucleic acid feature" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            has_clone,
            border_width,
            fill_color,
            |ctx, rect| {
                let radius = (rect.height * 0.3).max(1.0);
                path_round_bottom_rect_impl(ctx, rect.x0, rect.y0, rect.width, rect.height, radius)
            },
        ),
        "complex" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            has_clone,
            border_width,
            fill_color,
            |ctx, rect| {
                let corner = (rect.width.min(rect.height) * 0.2).max(1.0);
                path_cut_rect(ctx, rect, corner)
            },
        ),
        "perturbing agent" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            has_clone,
            border_width,
            fill_color,
            path_concave_hexagon,
        ),
        _ => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            has_clone,
            border_width,
            fill_color,
            path_rect,
        ),
    }
}

/// Map entity pool nodes to their fill colors, matching sbgnStyle defaults.
fn entity_pool_fill_color(class_name: &str) -> Option<(f64, f64, f64)> {
    match class_name {
        "complex" => Some(DEFAULT_FILL_COLOR),
        _ => Some(DEFAULT_FILL_COLOR),
    }
}

/// Return sbgnStyle border widths for entity pool nodes.
fn entity_pool_border_width(class_name: &str) -> f64 {
    match class_name {
        "complex" => 4.0,
        _ => 2.0,
    }
}

/// Return ghost offsets for multimer nodes, matching sbgnStyle values.
fn ghost_offset_for(class_name: &str) -> Option<(f64, f64)> {
    match class_name {
        "simple chemical" => Some((5.0, 5.0)),
        "macromolecule" | "nucleic acid feature" => Some((12.0, 12.0)),
        "complex" => Some((16.0, 16.0)),
        _ => None,
    }
}

/// Draw auxiliary overlays (clone markers, unit info, state vars) for entity pool nodes.
fn draw_entity_pool_aux_items(
    ctx: &CairoContext,
    rect: PixelRect,
    class_name: &str,
    u_info_label: Option<&str>,
    s_var_label: Option<&str>,
) -> Result<()> {
    // Auxiliary overlays (clone markers, unit info, state vars) are positioned in absolute
    // pixel space in sbgnStyle, so we scale them relative to the node's default dimensions.
    let (ref_w, ref_h) = default_dimensions(class_name).unwrap_or((rect.width, rect.height));
    let scale_x = rect.width / ref_w;
    let scale_y = rect.height / ref_h;
    let scale = (scale_x + scale_y) / 2.0;

    let aux_item_height = 20.0 * scale_y;
    let border_width = 2.0 * scale;
    let font_px = 10.0 * scale;
    let clone_shrink_y = 3.0 * scale_y;
    let u_info_height = aux_item_height - clone_shrink_y;

    match class_name {
        "simple chemical" => {
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 8.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 52.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if let Some(label) = u_info_label {
                let u_info_x = px_x(rect, 12.0, scale_x);
                let u_info_y = px_y(rect, 0.0, scale_y);
                draw_unit_info(
                    ctx,
                    u_info_x,
                    u_info_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    5.0 * scale,
                )?;
            }
        }
        "unspecified entity" => {
            if u_info_label.is_some() || s_var_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 8.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 52.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if let Some(label) = u_info_label {
                let u_info_x = px_x(rect, 20.0, scale_x);
                let u_info_y = px_y(rect, 44.0, scale_y);
                draw_unit_info(
                    ctx,
                    u_info_x,
                    u_info_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    5.0 * scale,
                )?;
            }
            if let Some(label) = s_var_label {
                let s_var_x = px_x(rect, 40.0, scale_x);
                let s_var_y = rect.y0;
                draw_state_var(
                    ctx,
                    s_var_x,
                    s_var_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    10.0 * scale,
                    30.0 * scale,
                )?;
            }
        }
        "macromolecule" => {
            if u_info_label.is_some() || s_var_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 8.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 52.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if let Some(label) = u_info_label {
                let u_info_x = px_x(rect, 20.0, scale_x);
                let u_info_y = px_y(rect, 44.0, scale_y);
                draw_unit_info(
                    ctx,
                    u_info_x,
                    u_info_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    5.0 * scale,
                )?;
            }
            if let Some(label) = s_var_label {
                let s_var_x = px_x(rect, 40.0, scale_x);
                let s_var_y = rect.y0;
                draw_state_var(
                    ctx,
                    s_var_x,
                    s_var_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    10.0 * scale,
                    30.0 * scale,
                )?;
            }
        }
        "nucleic acid feature" => {
            if s_var_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 8.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 52.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if let Some(label) = u_info_label {
                let u_info_x = px_x(rect, 20.0, scale_x);
                let u_info_y = px_y(rect, 44.0, scale_y);
                draw_unit_info(
                    ctx,
                    u_info_x,
                    u_info_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    5.0 * scale,
                )?;
            }
            if let Some(label) = s_var_label {
                let s_var_x = px_x(rect, 40.0, scale_x);
                let s_var_y = rect.y0;
                draw_state_var(
                    ctx,
                    s_var_x,
                    s_var_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    10.0 * scale,
                    30.0 * scale,
                )?;
            }
        }
        "complex" => {
            if u_info_label.is_some() || s_var_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 11.0, scale_y),
                    6.0 * scale,
                    BORDER_COLOR,
                )?;
            }
            if let Some(label) = u_info_label {
                let u_info_x = rect.x0 + rect.width * 0.25;
                let u_info_y = rect.y0;
                draw_unit_info(
                    ctx,
                    u_info_x,
                    u_info_y,
                    24.0 * scale_y - clone_shrink_y,
                    label,
                    border_width,
                    font_px,
                    5.0 * scale,
                )?;
            }
            if let Some(label) = s_var_label {
                let s_var_x = rect.x0 + rect.width * 0.88;
                let s_var_y = rect.y0;
                draw_state_var(
                    ctx,
                    s_var_x,
                    s_var_y,
                    24.0 * scale_y - clone_shrink_y,
                    label,
                    border_width,
                    font_px,
                    10.0 * scale,
                    30.0 * scale,
                )?;
            }
        }
        "perturbing agent" => {
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 8.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if u_info_label.is_some() {
                draw_overlay_line(
                    ctx,
                    rect,
                    px_y(rect, 56.0, scale_y),
                    1.0 * scale,
                    AUX_LINE_COLOR,
                )?;
            }
            if let Some(label) = u_info_label {
                let u_info_x = px_x(rect, 20.0, scale_x);
                let u_info_y = rect.y0;
                draw_unit_info(
                    ctx,
                    u_info_x,
                    u_info_y,
                    u_info_height,
                    label,
                    border_width,
                    font_px,
                    5.0 * scale,
                )?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Draw an orientation marker line for glyphs that define an orientation.
fn draw_orientation_marker(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    orientation: &str,
    connector_len_px: f64,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    match orientation {
        "vertical" => {
            ctx.new_path();
            ctx.move_to(rect.center.x, rect.y0 - connector_len_px);
            ctx.line_to(rect.center.x, rect.y0);
            ctx.move_to(rect.center.x, rect.y0 + rect.height);
            ctx.line_to(rect.center.x, rect.y0 + rect.height + connector_len_px);
            ctx.stroke()?;
        }
        "horizontal" => {
            ctx.new_path();
            ctx.move_to(rect.x0 - connector_len_px, rect.center.y);
            ctx.line_to(rect.x0, rect.center.y);
            ctx.move_to(rect.x0 + rect.width, rect.center.y);
            ctx.line_to(rect.x0 + rect.width + connector_len_px, rect.center.y);
            ctx.stroke()?;
        }
        "left" => {
            ctx.new_path();
            ctx.move_to(rect.x0 - connector_len_px, rect.center.y);
            ctx.line_to(rect.x0, rect.center.y);
            ctx.stroke()?;
        }
        "right" => {
            ctx.new_path();
            ctx.move_to(rect.x0 + rect.width, rect.center.y);
            ctx.line_to(rect.x0 + rect.width + connector_len_px, rect.center.y);
            ctx.stroke()?;
        }
        "up" => {
            ctx.new_path();
            ctx.move_to(rect.center.x, rect.y0 - connector_len_px);
            ctx.line_to(rect.center.x, rect.y0);
            ctx.stroke()?;
        }
        "down" => {
            ctx.new_path();
            ctx.move_to(rect.center.x, rect.y0 + rect.height);
            ctx.line_to(rect.center.x, rect.y0 + rect.height + connector_len_px);
            ctx.stroke()?;
        }
        _ => {}
    }
    Ok(())
}

/// Draw a horizontal overlay line at a specific y offset.
fn draw_overlay_line(
    ctx: &CairoContext,
    rect: PixelRect,
    y: f64,
    line_width: f64,
    color: (f64, f64, f64),
) -> Result<()> {
    ctx.set_line_width(line_width.max(1.0));
    ctx.set_source_rgb(color.0, color.1, color.2);
    ctx.new_path();
    ctx.move_to(rect.x0, y);
    ctx.line_to(rect.x0 + rect.width, y);
    ctx.stroke()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn port_connector_len_px_for_class(class_name: &str) -> f64 {
    if matches!(class_name, "and" | "or" | "not") {
        LOGICAL_PORT_CONNECTOR_LEN_PX
    } else {
        PORT_CONNECTOR_LEN_PX
    }
}

/// Draw a unit of information box sized from label width.
fn draw_unit_info(
    ctx: &CairoContext,
    x: f64,
    y: f64,
    height: f64,
    label: &str,
    border_width: f64,
    font_px: f64,
    padding_px: f64,
) -> Result<()> {
    let text_width = measure_text_width(ctx, label, font_px);
    let width = (text_width + padding_px).max(10.0);
    let rect = PixelRect {
        x0: x,
        y0: y,
        width,
        height,
        center: Point {
            x: x + width / 2.0,
            y: y + height / 2.0,
        },
    };
    ctx.set_line_width(border_width.max(1.0));
    path_round_rect_impl(ctx, rect.x0, rect.y0, rect.width, rect.height, rect.width * 0.04)?;
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px)?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

/// Draw a state variable box sized from label width.
fn draw_state_var(
    ctx: &CairoContext,
    x: f64,
    y: f64,
    height: f64,
    label: &str,
    border_width: f64,
    font_px: f64,
    padding_px: f64,
    min_width: f64,
) -> Result<()> {
    let text_width = measure_text_width(ctx, label, font_px);
    let width = (text_width + padding_px).max(min_width);
    let rect = PixelRect {
        x0: x,
        y0: y,
        width,
        height,
        center: Point {
            x: x + width / 2.0,
            y: y + height / 2.0,
        },
    };
    ctx.set_line_width(border_width.max(1.0));
    let radius = 0.24 * rect.width.max(rect.height);
    path_round_rect_impl(ctx, rect.x0, rect.y0, rect.width, rect.height, radius)?;
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px)?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

/// Measure label width using the current Cairo/Pango context.
fn measure_text_width(ctx: &CairoContext, text: &str, font_px: f64) -> f64 {
    let layout = pangocairo::create_layout(ctx);
    let mut font_desc = FontDescription::from_string(FONT_FAMILY);
    font_desc.set_absolute_size(font_px * pango::SCALE as f64);
    layout.set_font_description(Some(&font_desc));
    layout.set_text(text);
    let (width, _) = layout.pixel_size();
    width as f64
}

/// Convert an x offset in px units to the node's pixel space.
fn px_x(rect: PixelRect, value: f64, scale_x: f64) -> f64 {
    rect.x0 + value * scale_x
}

/// Convert a y offset in px units to the node's pixel space.
fn px_y(rect: PixelRect, value: f64, scale_y: f64) -> f64 {
    rect.y0 + value * scale_y
}


fn draw_circle_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
) -> Result<()> {
    let center = transform.map_point(bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
    let radius = transform.scale_scalar(bbox.w.min(bbox.h) / 2.0);
    ctx.arc(center.x, center.y, radius, 0.0, std::f64::consts::TAU);
    ctx.set_source_rgb(
        DEFAULT_FILL_COLOR.0,
        DEFAULT_FILL_COLOR.1,
        DEFAULT_FILL_COLOR.2,
    );
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    draw_text_centered(ctx, center, label, font_px)?;
    Ok(())
}

fn draw_shape_with_clone<F>(
    ctx: &CairoContext,
    rect: PixelRect,
    label: &str,
    font_px: f64,
    has_clone: bool,
    line_width: f64,
    fill_color: Option<(f64, f64, f64)>,
    path_fn: F,
) -> Result<()>
where
    F: Fn(&CairoContext, PixelRect) -> Result<()>,
{
    ctx.set_line_width(line_width.max(0.5));
    path_fn(ctx, rect)?;
    if let Some(color) = fill_color {
        ctx.set_source_rgb(color.0, color.1, color.2);
        ctx.fill_preserve()?;
    }
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    if has_clone {
        draw_clone_marker(ctx, rect, &path_fn)?;
        path_fn(ctx, rect)?;
        ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
        ctx.stroke()?;
    }
    draw_text_centered(ctx, rect.center, label, font_px)?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn draw_clone_marker<F>(ctx: &CairoContext, rect: PixelRect, path_fn: &F) -> Result<()>
where
    F: Fn(&CairoContext, PixelRect) -> Result<()>,
{
    let marker_height = (rect.height * CLONE_MARKER_HEIGHT_RATIO).max(1.0);
    let marker_width = rect.width;
    let marker_x = rect.center.x - marker_width / 2.0;
    let marker_y = rect.y0 + rect.height - marker_height;

    let _ = ctx.save();
    path_fn(ctx, rect)?;
    ctx.clip();
    ctx.new_path();
    ctx.rectangle(marker_x, marker_y, marker_width, marker_height);
    ctx.set_source_rgb(
        CLONE_MARKER_FILL_COLOR.0,
        CLONE_MARKER_FILL_COLOR.1,
        CLONE_MARKER_FILL_COLOR.2,
    );
    ctx.fill_preserve()?;
    ctx.set_source_rgb(AUX_LINE_COLOR.0, AUX_LINE_COLOR.1, AUX_LINE_COLOR.2);
    ctx.set_line_width(CLONE_MARKER_STROKE_WIDTH.max(1.0));
    ctx.stroke()?;
    let _ = ctx.restore();
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn path_rect(ctx: &CairoContext, rect: PixelRect) -> Result<()> {
    ctx.new_path();
    ctx.rectangle(rect.x0, rect.y0, rect.width, rect.height);
    Ok(())
}

fn path_ellipse(ctx: &CairoContext, rect: PixelRect) -> Result<()> {
    let radius_x = (rect.width / 2.0).max(1.0);
    let radius_y = (rect.height / 2.0).max(1.0);
    let _ = ctx.save();
    ctx.new_path();
    ctx.translate(rect.center.x, rect.center.y);
    ctx.scale(radius_x, radius_y);
    ctx.arc(0.0, 0.0, 1.0, 0.0, std::f64::consts::TAU);
    let _ = ctx.restore();
    Ok(())
}

fn path_round_rect(ctx: &CairoContext, rect: PixelRect, radius: f64) -> Result<()> {
    path_round_rect_impl(ctx, rect.x0, rect.y0, rect.width, rect.height, radius)
}

fn path_cut_rect(ctx: &CairoContext, rect: PixelRect, corner: f64) -> Result<()> {
    let x0 = rect.x0;
    let y0 = rect.y0;
    let x1 = rect.x0 + rect.width;
    let y1 = rect.y0 + rect.height;
    ctx.new_path();
    ctx.move_to(x0, y0 + corner);
    ctx.line_to(x0 + corner, y0);
    ctx.line_to(x1 - corner, y0);
    ctx.line_to(x1, y0 + corner);
    ctx.line_to(x1, y1 - corner);
    ctx.line_to(x1 - corner, y1);
    ctx.line_to(x0 + corner, y1);
    ctx.line_to(x0, y1 - corner);
    ctx.close_path();
    Ok(())
}

fn path_hexagon(ctx: &CairoContext, rect: PixelRect) -> Result<()> {
    let x0 = rect.x0;
    let y0 = rect.y0;
    let w = rect.width;
    let h = rect.height;
    let points = [
        Point {
            x: x0,
            y: y0 + 0.5 * h,
        },
        Point {
            x: x0 + 0.25 * w,
            y: y0,
        },
        Point {
            x: x0 + 0.75 * w,
            y: y0,
        },
        Point {
            x: x0 + w,
            y: y0 + 0.5 * h,
        },
        Point {
            x: x0 + 0.75 * w,
            y: y0 + h,
        },
        Point {
            x: x0 + 0.25 * w,
            y: y0 + h,
        },
    ];
    ctx.new_path();
    ctx.move_to(points[0].x, points[0].y);
    for point in &points[1..] {
        ctx.line_to(point.x, point.y);
    }
    ctx.close_path();
    Ok(())
}

fn path_concave_hexagon(ctx: &CairoContext, rect: PixelRect) -> Result<()> {
    let x0 = rect.x0;
    let y0 = rect.y0;
    let w = rect.width;
    let h = rect.height;
    let points = [
        Point { x: x0, y: y0 },
        Point { x: x0 + w, y: y0 },
        Point {
            x: x0 + 0.85 * w,
            y: y0 + 0.5 * h,
        },
        Point {
            x: x0 + w,
            y: y0 + h,
        },
        Point {
            x: x0,
            y: y0 + h,
        },
        Point {
            x: x0 + 0.15 * w,
            y: y0 + 0.5 * h,
        },
    ];
    ctx.new_path();
    ctx.move_to(points[0].x, points[0].y);
    for point in &points[1..] {
        ctx.line_to(point.x, point.y);
    }
    ctx.close_path();
    Ok(())
}

fn path_barrel(ctx: &CairoContext, rect: PixelRect) -> Result<()> {
    let x = rect.x0;
    let y = rect.y0;
    let w = rect.width;
    let h = rect.height;
    let top_y = y + 0.03 * h;
    let bottom_y = y + 0.97 * h;

    ctx.new_path();
    ctx.move_to(x, top_y);
    ctx.line_to(x, bottom_y);
    quad_curve_to(ctx, x + 0.06 * w, y + h, x + 0.25 * w, y + h)?;

    ctx.line_to(x + 0.75 * w, y + h);
    quad_curve_to(ctx, x + 0.95 * w, y + h, x + w, y + 0.95 * h)?;

    ctx.line_to(x + w, y + 0.05 * h);
    quad_curve_to(ctx, x + w, y, x + 0.75 * w, y)?;

    ctx.line_to(x + 0.25 * w, y);
    quad_curve_to(ctx, x + 0.06 * w, y, x, top_y)?;

    ctx.close_path();
    Ok(())
}

fn path_tag(ctx: &CairoContext, rect: PixelRect, notch: f64) -> Result<()> {
    let x0 = rect.x0;
    let y0 = rect.y0;
    let x1 = rect.x0 + rect.width;
    let y1 = rect.y0 + rect.height;
    let mid_y = (y0 + y1) / 2.0;
    ctx.new_path();
    ctx.move_to(x0 + notch, y0);
    ctx.line_to(x1, y0);
    ctx.line_to(x1, y1);
    ctx.line_to(x0 + notch, y1);
    ctx.line_to(x0, mid_y);
    ctx.close_path();
    Ok(())
}

fn path_round_rect_impl(
    ctx: &CairoContext,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) -> Result<()> {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    let right = x + width;
    let bottom = y + height;

    ctx.new_path();
    ctx.move_to(x + radius, y);
    ctx.line_to(right - radius, y);
    ctx.arc(right - radius, y + radius, radius, -std::f64::consts::FRAC_PI_2, 0.0);
    ctx.line_to(right, bottom - radius);
    ctx.arc(
        right - radius,
        bottom - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    ctx.line_to(x + radius, bottom);
    ctx.arc(
        x + radius,
        bottom - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    ctx.line_to(x, y + radius);
    ctx.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        std::f64::consts::FRAC_PI_2 * 3.0,
    );
    ctx.close_path();
    Ok(())
}

fn path_round_bottom_rect_impl(
    ctx: &CairoContext,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    radius: f64,
) -> Result<()> {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    let right = x + width;
    let bottom = y + height;

    ctx.new_path();
    ctx.move_to(x, y);
    ctx.line_to(right, y);
    ctx.line_to(right, bottom - radius);
    ctx.arc(
        right - radius,
        bottom - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    ctx.line_to(x + radius, bottom);
    ctx.arc(
        x + radius,
        bottom - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    ctx.close_path();
    Ok(())
}

fn quad_curve_to(ctx: &CairoContext, cx: f64, cy: f64, x: f64, y: f64) -> Result<()> {
    let (x0, y0) = ctx
        .current_point()
        .context("Missing current point for quadratic curve")?;
    let c1x = x0 + 2.0 / 3.0 * (cx - x0);
    let c1y = y0 + 2.0 / 3.0 * (cy - y0);
    let c2x = x + 2.0 / 3.0 * (cx - x);
    let c2y = y + 2.0 / 3.0 * (cy - y);
    ctx.curve_to(c1x, c1y, c2x, c2y, x, y);
    Ok(())
}

fn draw_arc(
    ctx: &CairoContext,
    points: &[Point],
    class_name: &str,
    arrow_size: f64,
    bar_length: f64,
    bar_offset: f64,
) -> Result<()> {
    if points.len() < 2 {
        return Ok(());
    }

    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    for pair in points.windows(2) {
        ctx.move_to(pair[0].x, pair[0].y);
        ctx.line_to(pair[1].x, pair[1].y);
        ctx.stroke()?;
    }

    let end = points[points.len() - 1];
    let prev = points[points.len() - 2];

    match class_name {
        "assignment" | "unknown influence" => {
            draw_open_triangle(ctx, end, prev, arrow_size)?
        }
        "positive influence" | "stimulation" => {
            draw_open_triangle_opaque(ctx, end, prev, arrow_size)?
        }
        "production" => draw_filled_triangle(ctx, end, prev, arrow_size)?,
        "negative influence" | "inhibition" => {
            draw_inhibition_bar(ctx, end, prev, bar_length, 0.0)?
        }
        "absolute inhibition" => {
            draw_inhibition_bar(ctx, end, prev, bar_length, 0.0)?;
            draw_inhibition_bar(ctx, end, prev, bar_length, bar_offset)?;
        }
        "necessary stimulation" => {
            draw_inhibition_bar(ctx, end, prev, bar_length, bar_offset)?;
            draw_open_triangle_opaque(ctx, end, prev, arrow_size)?;
        }
        "catalysis" => draw_filled_circle_tangent(ctx, end, prev, arrow_size * 0.4)?,
        "equivalence arc" => draw_open_circle(ctx, end, arrow_size * 0.4)?,
        _ => {}
    }

    Ok(())
}

fn draw_open_circle(ctx: &CairoContext, center: Point, radius: f64) -> Result<()> {
    ctx.arc(center.x, center.y, radius.max(1.0), 0.0, std::f64::consts::TAU);
    ctx.stroke()?;
    Ok(())
}

fn draw_filled_circle(ctx: &CairoContext, center: Point, radius: f64) -> Result<()> {
    ctx.arc(center.x, center.y, radius.max(1.0), 0.0, std::f64::consts::TAU);
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    Ok(())
}

fn draw_filled_circle_tangent(
    ctx: &CairoContext,
    end: Point,
    prev: Point,
    radius: f64,
) -> Result<()> {
    let dx = end.x - prev.x;
    let dy = end.y - prev.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return draw_filled_circle(ctx, end, radius);
    }
    let ux = dx / len;
    let uy = dy / len;
    let overlap = (radius * CATALYSIS_OVERLAP_RATIO).max(0.0);
    let offset = (radius - overlap).max(0.0);
    let center = Point {
        x: end.x - ux * offset,
        y: end.y - uy * offset,
    };
    draw_filled_circle(ctx, center, radius)
}

fn draw_open_triangle(ctx: &CairoContext, end: Point, prev: Point, size: f64) -> Result<()> {
    let Some((p1, p2, tip)) = triangle_points(end, prev, size) else {
        return Ok(());
    };
    ctx.move_to(p1.x, p1.y);
    ctx.line_to(p2.x, p2.y);
    ctx.line_to(tip.x, tip.y);
    ctx.close_path();
    ctx.stroke()?;
    Ok(())
}

fn draw_open_triangle_opaque(
    ctx: &CairoContext,
    end: Point,
    prev: Point,
    size: f64,
) -> Result<()> {
    let Some((p1, p2, tip)) = triangle_points(end, prev, size) else {
        return Ok(());
    };
    ctx.move_to(p1.x, p1.y);
    ctx.line_to(p2.x, p2.y);
    ctx.line_to(tip.x, tip.y);
    ctx.close_path();
    ctx.set_source_rgb(1.0, 1.0, 1.0);
    ctx.fill_preserve()?;
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.stroke()?;
    Ok(())
}

fn draw_filled_triangle(ctx: &CairoContext, end: Point, prev: Point, size: f64) -> Result<()> {
    let Some((p1, p2, tip)) = triangle_points(end, prev, size) else {
        return Ok(());
    };
    ctx.move_to(p1.x, p1.y);
    ctx.line_to(p2.x, p2.y);
    ctx.line_to(tip.x, tip.y);
    ctx.close_path();
    ctx.fill()?;
    Ok(())
}

fn triangle_points(end: Point, prev: Point, size: f64) -> Option<(Point, Point, Point)> {
    let dx = end.x - prev.x;
    let dy = end.y - prev.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        return None;
    }
    let ux = dx / length;
    let uy = dy / length;
    let base_x = end.x - ux * size;
    let base_y = end.y - uy * size;
    let perp_x = -uy;
    let perp_y = ux;
    let half_width = size * 0.6;
    let p1 = Point {
        x: base_x + perp_x * half_width,
        y: base_y + perp_y * half_width,
    };
    let p2 = Point {
        x: base_x - perp_x * half_width,
        y: base_y - perp_y * half_width,
    };
    Some((p1, p2, end))
}

fn draw_inhibition_bar(
    ctx: &CairoContext,
    end: Point,
    prev: Point,
    length: f64,
    offset: f64,
) -> Result<()> {
    let dx = end.x - prev.x;
    let dy = end.y - prev.y;
    let seg_len = (dx * dx + dy * dy).sqrt();
    if seg_len == 0.0 {
        return Ok(());
    }
    let ux = dx / seg_len;
    let uy = dy / seg_len;
    let center_x = end.x - ux * offset;
    let center_y = end.y - uy * offset;
    let perp_x = -uy;
    let perp_y = ux;
    let half_len = length / 2.0;
    let p0 = Point {
        x: center_x - perp_x * half_len,
        y: center_y - perp_y * half_len,
    };
    let p1 = Point {
        x: center_x + perp_x * half_len,
        y: center_y + perp_y * half_len,
    };
    ctx.move_to(p0.x, p0.y);
    ctx.line_to(p1.x, p1.y);
    ctx.stroke()?;
    Ok(())
}

fn draw_text_centered(ctx: &CairoContext, center: Point, text: &str, font_px: f64) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let layout = pangocairo::create_layout(ctx);
    let mut font_desc = FontDescription::from_string(FONT_FAMILY);
    font_desc.set_absolute_size(font_px * pango::SCALE as f64);
    layout.set_font_description(Some(&font_desc));
    layout.set_alignment(Alignment::Center);
    layout.set_text(text);

    let (width, height) = layout.pixel_size();
    let x = center.x - width as f64 / 2.0;
    let y = center.y - height as f64 / 2.0;
    draw_text_at(ctx, x, y, &layout)?;
    Ok(())
}

/// Draw text with an outline at the given top-left position.
fn draw_text_at(ctx: &CairoContext, x: f64, y: f64, layout: &pango::Layout) -> Result<()> {
    ctx.move_to(x, y);
    pangocairo::layout_path(ctx, layout);
    if TEXT_OUTLINE_WIDTH > 0.0 {
        ctx.set_source_rgb(1.0, 1.0, 1.0);
        ctx.set_line_width(TEXT_OUTLINE_WIDTH);
        ctx.stroke_preserve()?;
    }
    ctx.set_source_rgb(BORDER_COLOR.0, BORDER_COLOR.1, BORDER_COLOR.2);
    ctx.fill()?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

/// Draw text aligned to the bottom-center of a bounding rectangle.
fn draw_text_bottom_centered(
    ctx: &CairoContext,
    rect: PixelRect,
    text: &str,
    font_px: f64,
) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let layout = pangocairo::create_layout(ctx);
    let mut font_desc = FontDescription::from_string(FONT_FAMILY);
    font_desc.set_absolute_size(font_px * pango::SCALE as f64);
    layout.set_font_description(Some(&font_desc));
    layout.set_alignment(Alignment::Center);
    layout.set_text(text);

    let (width, height) = layout.pixel_size();
    let x = rect.center.x - width as f64 / 2.0;
    let y = rect.y0 + rect.height - height as f64 - 2.0;
    draw_text_at(ctx, x, y, &layout)
}
fn bbox_pixel_rect(transform: &Transform, bbox: BBox) -> PixelRect {
    let x0 = (bbox.x - transform.min_x) * transform.scale_x;
    let x1 = (bbox.x + bbox.w - transform.min_x) * transform.scale_x;
    let y0 = (bbox.y - transform.min_y) * transform.scale_y;
    let y1 = (bbox.y + bbox.h - transform.min_y) * transform.scale_y;
    let left = x0.min(x1);
    let right = x0.max(x1);
    let top = y0.min(y1);
    let bottom = y0.max(y1);
    PixelRect {
        x0: left,
        y0: top,
        width: right - left,
        height: bottom - top,
        center: Point {
            x: (left + right) / 2.0,
            y: (top + bottom) / 2.0,
        },
    }
}

/// Build a state variable label in the same format as sbgnStyle (value@variable).
fn state_var_label(value: Option<&str>, variable: Option<&str>) -> String {
    match (value, variable) {
        (Some(value), Some(variable)) if !value.is_empty() && !variable.is_empty() => {
            format!("{value}@{variable}")
        }
        (Some(value), _) if !value.is_empty() => value.to_string(),
        (_, Some(variable)) if !variable.is_empty() => variable.to_string(),
        _ => String::new(),
    }
}

fn first_child_label(children: &[&Glyph], class_name: &str) -> Option<String> {
    children
        .iter()
        .find(|child| child.class_name == class_name)
        .map(|child| child.label.clone())
        .filter(|label| !label.trim().is_empty())
}

fn first_child_state_label(children: &[&Glyph], class_name: &str) -> Option<String> {
    children
        .iter()
        .find(|child| child.class_name == class_name)
        .map(|child| {
            if !child.label.trim().is_empty() {
                child.label.clone()
            } else {
                state_var_label(
                    child.state_value.as_deref(),
                    child.state_variable.as_deref(),
                )
            }
        })
        .filter(|label| !label.trim().is_empty())
}

/// Return default widths/heights from sbgnStyle for scale reference.
fn default_dimensions(class_name: &str) -> Option<(f64, f64)> {
    match class_name {
        "unspecified entity" => Some((32.0, 32.0)),
        "simple chemical" | "simple chemical multimer" => Some((48.0, 48.0)),
        "macromolecule" | "macromolecule multimer" => Some((96.0, 48.0)),
        "nucleic acid feature" => Some((88.0, 56.0)),
        "nucleic acid feature multimer" => Some((88.0, 52.0)),
        "complex" | "complex multimer" => Some((10.0, 10.0)),
        "source and sink" => Some((60.0, 60.0)),
        "perturbing agent" => Some((140.0, 60.0)),
        "phenotype" => Some((140.0, 60.0)),
        "process" | "uncertain process" | "omitted process" => Some((25.0, 25.0)),
        "association" | "dissociation" => Some((25.0, 25.0)),
        "compartment" => Some((50.0, 50.0)),
        "tag" => Some((100.0, 65.0)),
        "and" | "or" | "not" => Some((40.0, 40.0)),
        _ => None,
    }
}

fn glyph_font_px(class_name: &str) -> f64 {
    match class_name {
        "state variable"
        | "unit of information"
        | "cardinality"
        | "variable value"
        | "tag"
        | "terminal" => FONT_SMALL_PX,
        _ => FONT_MAIN_PX,
    }
}

fn parse_sbgn(doc: &Document) -> Result<(Vec<Glyph>, Vec<Arc>, Bounds)> {
    let arc_nodes: Vec<_> = doc
        .descendants()
        .filter(|node| node.has_tag_name("arc"))
        .collect();

    let mut glyphs = Vec::new();
    let map_node = doc
        .descendants()
        .find(|node| node.has_tag_name("map"))
        .ok_or_else(|| anyhow!("SBGN file missing map element"))?;
    for glyph_node in map_node.children().filter(|node| node.has_tag_name("glyph")) {
        parse_glyph_node(&glyph_node, None, &mut glyphs)?;
    }

    let mut arcs = Vec::new();
    for arc in arc_nodes {
        let class_name = arc
            .attribute("class")
            .unwrap_or_default()
            .to_string();
        let start = arc
            .children()
            .find(|node| node.has_tag_name("start"))
            .ok_or_else(|| anyhow!("Arc missing start"))?;
        let end = arc
            .children()
            .find(|node| node.has_tag_name("end"))
            .ok_or_else(|| anyhow!("Arc missing end"))?;

        let mut points = Vec::new();
        points.push(Point {
            x: parse_f64(start.attribute("x")).ok_or_else(|| anyhow!("Bad arc start x"))?,
            y: parse_f64(start.attribute("y")).ok_or_else(|| anyhow!("Bad arc start y"))?,
        });

        for next in arc.children().filter(|node| node.has_tag_name("next")) {
            if let (Some(x), Some(y)) = (parse_f64(next.attribute("x")), parse_f64(next.attribute("y"))) {
                points.push(Point { x, y });
            }
        }

        points.push(Point {
            x: parse_f64(end.attribute("x")).ok_or_else(|| anyhow!("Bad arc end x"))?,
            y: parse_f64(end.attribute("y")).ok_or_else(|| anyhow!("Bad arc end y"))?,
        });

        arcs.push(Arc { class_name, points });
    }

    let bounds = compute_bounds(&glyphs, &arcs)?;
    Ok((glyphs, arcs, bounds))
}

fn parse_glyph_node(
    glyph: &roxmltree::Node,
    parent_id: Option<String>,
    glyphs: &mut Vec<Glyph>,
) -> Result<()> {
    // Walk the SBGN XML tree recursively so child glyphs (units, state vars) keep their parent.
    let id = glyph.attribute("id").unwrap_or_default().to_string();
    let class_name = glyph
        .attribute("class")
        .unwrap_or_default()
        .to_string();
    let label_node = glyph
        .children()
        .find(|node| node.has_tag_name("label"));
    let mut label = label_node
        .and_then(|node| node.attribute("text"))
        .unwrap_or("")
        .to_string();
    label = label.replace('\r', "");

    let bbox_node = glyph.children().find(|node| node.has_tag_name("bbox"));
    let bbox = bbox_node.and_then(|node| parse_bbox(&node));

    let ports = glyph
        .children()
        .filter(|node| node.has_tag_name("port"))
        .filter_map(|node| {
            let x = parse_f64(node.attribute("x"))?;
            let y = parse_f64(node.attribute("y"))?;
            Some(Point { x, y })
        })
        .collect();

    let has_clone = glyph.children().any(|node| node.has_tag_name("clone"));
    let state_node = glyph.children().find(|node| node.has_tag_name("state"));
    let state_value = state_node
        .and_then(|node| node.attribute("value"))
        .map(|value| value.to_string());
    let state_variable = state_node
        .and_then(|node| node.attribute("variable"))
        .map(|value| value.to_string());
    let orientation = glyph.attribute("orientation").map(|value| value.to_string());

    let glyph_id = id.clone();
    glyphs.push(Glyph {
        id,
        parent_id,
        class_name,
        bbox,
        label,
        ports,
        has_clone,
        state_value,
        state_variable,
        orientation,
    });

    for child in glyph.children().filter(|node| node.has_tag_name("glyph")) {
        parse_glyph_node(&child, Some(glyph_id.clone()), glyphs)?;
    }
    Ok(())
}

fn parse_bbox(node: &roxmltree::Node) -> Option<BBox> {
    Some(BBox {
        x: parse_f64(node.attribute("x"))?,
        y: parse_f64(node.attribute("y"))?,
        w: parse_f64(node.attribute("w"))?,
        h: parse_f64(node.attribute("h"))?,
    })
}

fn parse_f64(value: Option<&str>) -> Option<f64> {
    value.and_then(|v| v.parse::<f64>().ok())
}

fn compute_bounds(glyphs: &[Glyph], _arcs: &[Arc]) -> Result<Bounds> {
    let mut x_values = Vec::new();
    let mut y_values = Vec::new();

    for glyph in glyphs {
        if let Some(bbox) = glyph.bbox {
            x_values.push(bbox.x);
            x_values.push(bbox.x + bbox.w);
            y_values.push(bbox.y);
            y_values.push(bbox.y + bbox.h);
        }
        for port in &glyph.ports {
            x_values.push(port.x);
            y_values.push(port.y);
        }
    }

    if x_values.is_empty() || y_values.is_empty() {
        return Err(anyhow!("No coordinates found in SBGN file"));
    }

    Ok(Bounds {
        min_x: x_values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min),
        max_x: x_values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
        min_y: y_values
            .iter()
            .copied()
            .fold(f64::INFINITY, f64::min),
        max_y: y_values
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max),
    })
}

/// Compute a padded transform and canvas size from data bounds.
fn transform_with_padding(bounds: Bounds, padding: f64) -> (Transform, f64, f64) {
    // Expand the data bounds so rendered output includes a consistent pixel margin.
    let min_x = bounds.min_x - padding;
    let max_x = bounds.max_x + padding;
    let min_y = bounds.min_y - padding;
    let max_y = bounds.max_y + padding;
    let width = (max_x - min_x).abs().max(1.0);
    let height = (max_y - min_y).abs().max(1.0);
    (
        Transform::new(min_x, min_y, max_x, max_y, width, height),
        width,
        height,
    )
}
