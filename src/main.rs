use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use cairo::{Context as CairoContext, Format, ImageSurface, LineCap, SvgSurface};
use clap::{Parser, Subcommand};
use pango::{Alignment, FontDescription};
use pangocairo::functions as pangocairo;
use xmltree::{Element, XMLNode};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Color {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

impl Color {
    const fn rgb(r: f64, g: f64, b: f64) -> Self {
        Self { r, g, b, a: 1.0 }
    }

    const fn rgba(r: f64, g: f64, b: f64, a: f64) -> Self {
        Self { r, g, b, a }
    }
}

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
const BORDER_COLOR: Color = Color::rgb(
    0x55 as f64 / 255.0,
    0x55 as f64 / 255.0,
    0x55 as f64 / 255.0,
);
const DEFAULT_FILL_COLOR: Color = Color::rgb(
    0xF6 as f64 / 255.0,
    0xF6 as f64 / 255.0,
    0xF6 as f64 / 255.0,
);
const AUX_LINE_COLOR: Color = Color::rgb(
    0x6A as f64 / 255.0,
    0x6A as f64 / 255.0,
    0x6A as f64 / 255.0,
);
const ASSOCIATION_FILL_COLOR: Color = Color::rgb(
    0x6B as f64 / 255.0,
    0x6B as f64 / 255.0,
    0x6B as f64 / 255.0,
);
const CLONE_MARKER_HEIGHT_RATIO: f64 = 0.30;
const CLONE_MARKER_FILL_COLOR: Color = Color::rgb(0.82, 0.82, 0.82);
const CLONE_MARKER_STROKE_WIDTH: f64 = 1.5;
const WHITE_COLOR: Color = Color::rgb(1.0, 1.0, 1.0);

#[derive(Parser)]
#[command(
    author,
    version,
    about = "Render SBGNML diagrams to PNG and SVG",
    long_about = None
)]
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
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, default_value = "png,svg")]
        format: String,
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
    id: String,
    class_name: String,
    source: Option<String>,
    target: Option<String>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputFormat {
    Png,
    Svg,
}

#[derive(Clone, Copy, Debug)]
enum TagOrientation {
    Left,
    Right,
}

impl OutputFormat {
    fn extension(self) -> &'static str {
        match self {
            OutputFormat::Png => "png",
            OutputFormat::Svg => "svg",
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RenderStyle {
    font_size: Option<f64>,
    font_family: Option<String>,
    font_color: Option<Color>,
    stroke_color: Option<Color>,
    stroke_width: Option<f64>,
    fill_color: Option<Color>,
    background_opacity: Option<f64>,
}

#[derive(Clone, Debug, Default)]
struct RenderInfo {
    background_color: Option<Color>,
    default_style: Option<RenderStyle>,
    styles: HashMap<String, RenderStyle>,
    colors: HashMap<String, Color>,
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
            format,
            padding,
            clone_markers,
        } => {
            let output_base = output.unwrap_or_else(|| input.clone());
            let formats = parse_output_formats(&format)?;
            draw_sbgnml(&input, &output_base, &formats, padding, clone_markers)
        }
    }
}

fn set_source_color(ctx: &CairoContext, color: Color) {
    ctx.set_source_rgba(color.r, color.g, color.b, color.a);
}

fn setup_context(ctx: &CairoContext, background: Option<Color>) -> Result<()> {
    let bg_color = background
        .filter(|color| color.a > 0.0)
        .unwrap_or(WHITE_COLOR);
    set_source_color(ctx, bg_color);
    ctx.paint()?;
    set_source_color(ctx, BORDER_COLOR);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    ctx.set_line_cap(LineCap::Square);
    Ok(())
}

fn create_png_surface(
    width: i32,
    height: i32,
    background: Option<Color>,
) -> Result<(ImageSurface, CairoContext)> {
    let surface = ImageSurface::create(Format::ARgb32, width, height)
        .context("Failed to create image surface")?;
    let ctx = CairoContext::new(&surface).context("Failed to create Cairo context")?;
    setup_context(&ctx, background)?;
    Ok((surface, ctx))
}

fn render_svg<F>(
    svg_path: &Path,
    width: f64,
    height: f64,
    background: Option<Color>,
    render: F,
) -> Result<()>
where
    F: FnOnce(&CairoContext) -> Result<()>,
{
    let surface =
        SvgSurface::new(width, height, Some(svg_path)).context("Failed to create SVG surface")?;
    let ctx = CairoContext::new(&surface).context("Failed to create Cairo context")?;
    setup_context(&ctx, background)?;
    render(&ctx)?;
    surface.finish();
    Ok(())
}

fn draw_sbgnml(
    input: &Path,
    output_base: &Path,
    formats: &[OutputFormat],
    padding: f64,
    show_clone_markers: bool,
) -> Result<()> {
    let xml = fs::read_to_string(input).with_context(|| format!("Failed to read {:?}", input))?;
    let root = Element::parse(xml.as_bytes()).context("Failed to parse SBGN XML")?;
    let render_info = parse_render_information(&root);
    let (glyphs, arcs, bounds) = parse_sbgn(&root)?;
    let tag_orientations = compute_tag_orientations(&glyphs, &arcs);

    let (transform, width_f, height_f) = transform_with_padding(bounds, padding);
    if formats.contains(&OutputFormat::Png) {
        let png_path = output_path_for_format(output_base, OutputFormat::Png);
        let (surface, ctx) = create_png_surface(
            width_f.ceil() as i32,
            height_f.ceil() as i32,
            render_info.background_color,
        )?;
        render_sbgnml(
            &ctx,
            &transform,
            &glyphs,
            &arcs,
            &render_info,
            &tag_orientations,
            show_clone_markers,
        )?;

        let mut file = fs::File::create(&png_path).context("Failed to create PNG file")?;
        surface
            .write_to_png(&mut file)
            .context("Failed to write PNG")?;
    }

    if formats.contains(&OutputFormat::Svg) {
        let svg_path = output_path_for_format(output_base, OutputFormat::Svg);
        render_svg(
            &svg_path,
            width_f,
            height_f,
            render_info.background_color,
            |ctx| {
                render_sbgnml(
                    ctx,
                    &transform,
                    &glyphs,
                    &arcs,
                    &render_info,
                    &tag_orientations,
                    show_clone_markers,
                )
            },
        )?;
    }
    Ok(())
}

fn output_path_for_format(output_base: &Path, format: OutputFormat) -> PathBuf {
    let mut path = output_base.to_path_buf();
    path.set_extension(format.extension());
    path
}

fn parse_output_formats(value: &str) -> Result<Vec<OutputFormat>> {
    let mut formats = Vec::new();
    for raw in value.split(',') {
        let normalized = raw.trim().to_lowercase();
        if normalized.is_empty() {
            continue;
        }
        let format = match normalized.as_str() {
            "png" => OutputFormat::Png,
            "svg" => OutputFormat::Svg,
            _ => {
                return Err(anyhow!(
                    "Unsupported format '{normalized}'. Use --format png,svg"
                ))
            }
        };
        if !formats.contains(&format) {
            formats.push(format);
        }
    }

    if formats.is_empty() {
        return Err(anyhow!("No output formats specified. Use --format png,svg"));
    }

    Ok(formats)
}

fn compute_tag_orientations(glyphs: &[Glyph], arcs: &[Arc]) -> HashMap<String, TagOrientation> {
    let mut orientations = HashMap::new();
    for glyph in glyphs.iter().filter(|item| item.class_name == "tag") {
        let Some(bbox) = glyph.bbox else {
            continue;
        };
        let mut best: Option<(f64, TagOrientation)> = None;
        for arc in arcs {
            let (matches, point) = match_arc_connection(arc, &glyph.id);
            if !matches {
                continue;
            }
            let Some(point) = point else {
                continue;
            };
            let dist_left = (point.x - bbox.x).abs();
            let dist_right = (point.x - (bbox.x + bbox.w)).abs();
            let (dist, orientation) = if dist_left <= dist_right {
                (dist_left, TagOrientation::Left)
            } else {
                (dist_right, TagOrientation::Right)
            };
            match best {
                Some((best_dist, _)) if best_dist <= dist => {}
                _ => best = Some((dist, orientation)),
            }
        }
        let orientation = best.map(|item| item.1).unwrap_or(TagOrientation::Left);
        orientations.insert(glyph.id.clone(), orientation);
    }
    orientations
}

fn compute_default_label_font_px(glyphs: &[Glyph], render_info: &RenderInfo) -> Option<f64> {
    let mut counts: HashMap<i64, (usize, f64)> = HashMap::new();
    for glyph in glyphs {
        if matches!(
            glyph.class_name.as_str(),
            "unit of information"
                | "state variable"
                | "and"
                | "or"
                | "not"
                | "delay"
                | "omitted process"
                | "uncertain process"
        ) {
            continue;
        }
        let style = merged_style(
            render_info.styles.get(&glyph.id),
            render_info.default_style.as_ref(),
        );
        let size = resolve_font_px(&style, glyph_font_px(glyph.class_name.as_str()));
        if size <= 0.0 {
            continue;
        }
        let bucket = (size * 10.0).round() as i64;
        let entry = counts.entry(bucket).or_insert((0, size));
        entry.0 += 1;
        entry.1 = size;
    }
    counts
        .into_values()
        .max_by_key(|(count, _)| *count)
        .map(|(_, size)| size)
}

fn match_arc_connection(arc: &Arc, glyph_id: &str) -> (bool, Option<Point>) {
    let source_matches = arc
        .source
        .as_deref()
        .map(|value| arc_ref_matches(value, glyph_id))
        .unwrap_or(false);
    if source_matches {
        return (true, arc.points.first().copied());
    }
    let target_matches = arc
        .target
        .as_deref()
        .map(|value| arc_ref_matches(value, glyph_id))
        .unwrap_or(false);
    if target_matches {
        return (true, arc.points.last().copied());
    }
    (false, None)
}

fn arc_ref_matches(arc_ref: &str, glyph_id: &str) -> bool {
    arc_ref == glyph_id || arc_ref.starts_with(&format!("{glyph_id}."))
}

fn element_attr<'a>(element: &'a Element, name: &str) -> Option<&'a str> {
    element.attributes.get(name).map(String::as_str)
}

fn child_elements<'a>(element: &'a Element) -> impl Iterator<Item = &'a Element> {
    element.children.iter().filter_map(|node| match node {
        XMLNode::Element(child) => Some(child),
        _ => None,
    })
}

fn find_first_descendant<'a>(element: &'a Element, name: &str) -> Option<&'a Element> {
    for child in child_elements(element) {
        if child.name == name {
            return Some(child);
        }
        if let Some(found) = find_first_descendant(child, name) {
            return Some(found);
        }
    }
    None
}

fn collect_descendants_by_name<'a>(element: &'a Element, name: &str, out: &mut Vec<&'a Element>) {
    for child in child_elements(element) {
        if child.name == name {
            out.push(child);
        }
        collect_descendants_by_name(child, name, out);
    }
}

fn parse_render_information(root: &Element) -> RenderInfo {
    let mut info = RenderInfo::default();
    let Some(render_node) = find_first_descendant(root, "renderInformation") else {
        return info;
    };

    let mut color_defs = Vec::new();
    collect_descendants_by_name(render_node, "colorDefinition", &mut color_defs);
    for color_def in color_defs {
        let Some(id) = element_attr(color_def, "id") else {
            continue;
        };
        let Some(value) = element_attr(color_def, "value") else {
            continue;
        };
        if let Some(color) = parse_color_value(value, &info.colors) {
            info.colors.insert(id.to_string(), color);
        }
    }

    info.background_color = render_node
        .attributes
        .get("background-color")
        .map(String::as_str)
        .and_then(|value| parse_color_value(value, &info.colors));

    let mut style_nodes = Vec::new();
    collect_descendants_by_name(render_node, "style", &mut style_nodes);
    for style_node in style_nodes {
        let g_node = child_elements(style_node).find(|node| node.name == "g");
        let mut style = RenderStyle::default();
        if let Some(g_node) = g_node {
            style.font_size = g_node
                .attributes
                .get("font-size")
                .map(String::as_str)
                .and_then(|value| parse_f64(Some(value)));
            style.font_family = g_node
                .attributes
                .get("font-family")
                .map(|value| value.to_string());
            style.font_color = g_node
                .attributes
                .get("font-color")
                .map(String::as_str)
                .and_then(|value| parse_color_value(value, &info.colors));
            style.stroke_color = g_node
                .attributes
                .get("stroke")
                .map(String::as_str)
                .and_then(|value| parse_color_value(value, &info.colors));
            style.stroke_width = g_node
                .attributes
                .get("stroke-width")
                .map(String::as_str)
                .and_then(|value| parse_f64(Some(value)));
            style.fill_color = g_node
                .attributes
                .get("fill")
                .map(String::as_str)
                .and_then(|value| parse_color_value(value, &info.colors));
            style.background_opacity = g_node
                .attributes
                .get("background-opacity")
                .map(String::as_str)
                .and_then(|value| parse_f64(Some(value)));
        }

        let id_list = style_node
            .attributes
            .get("idList")
            .map(String::as_str)
            .unwrap_or("");
        let ids: Vec<&str> = id_list.split_whitespace().collect();
        if ids.is_empty() {
            info.default_style = Some(style);
        } else {
            for id in ids {
                info.styles.insert(id.to_string(), style.clone());
            }
        }
    }

    info
}

fn parse_color_value(value: &str, colors: &HashMap<String, Color>) -> Option<Color> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return None;
    }
    if let Some(color) = colors.get(trimmed) {
        return Some(*color);
    }
    if trimmed.starts_with('#') {
        return parse_hex_color(trimmed);
    }
    None
}

fn parse_hex_color(value: &str) -> Option<Color> {
    let hex = value.trim().trim_start_matches('#');
    let (r, g, b, a) = match hex.len() {
        3 => {
            let r = parse_hex_nibble(hex.chars().nth(0)?)?;
            let g = parse_hex_nibble(hex.chars().nth(1)?)?;
            let b = parse_hex_nibble(hex.chars().nth(2)?)?;
            (r, g, b, 0xFF)
        }
        4 => {
            let r = parse_hex_nibble(hex.chars().nth(0)?)?;
            let g = parse_hex_nibble(hex.chars().nth(1)?)?;
            let b = parse_hex_nibble(hex.chars().nth(2)?)?;
            let a = parse_hex_nibble(hex.chars().nth(3)?)?;
            (r, g, b, a)
        }
        6 => {
            let r = parse_hex_byte(&hex[0..2])?;
            let g = parse_hex_byte(&hex[2..4])?;
            let b = parse_hex_byte(&hex[4..6])?;
            (r, g, b, 0xFF)
        }
        8 => {
            let r = parse_hex_byte(&hex[0..2])?;
            let g = parse_hex_byte(&hex[2..4])?;
            let b = parse_hex_byte(&hex[4..6])?;
            let a = parse_hex_byte(&hex[6..8])?;
            (r, g, b, a)
        }
        _ => return None,
    };
    Some(Color::rgba(
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        a as f64 / 255.0,
    ))
}

fn parse_hex_byte(value: &str) -> Option<u8> {
    u8::from_str_radix(value, 16).ok()
}

fn parse_hex_nibble(value: char) -> Option<u8> {
    let digit = value.to_digit(16)? as u8;
    Some(digit * 17)
}

fn merged_style(primary: Option<&RenderStyle>, fallback: Option<&RenderStyle>) -> RenderStyle {
    RenderStyle {
        font_size: primary
            .and_then(|style| style.font_size)
            .or_else(|| fallback.and_then(|style| style.font_size)),
        font_family: primary
            .and_then(|style| style.font_family.clone())
            .or_else(|| fallback.and_then(|style| style.font_family.clone())),
        font_color: primary
            .and_then(|style| style.font_color)
            .or_else(|| fallback.and_then(|style| style.font_color)),
        stroke_color: primary
            .and_then(|style| style.stroke_color)
            .or_else(|| fallback.and_then(|style| style.stroke_color)),
        stroke_width: primary
            .and_then(|style| style.stroke_width)
            .or_else(|| fallback.and_then(|style| style.stroke_width)),
        fill_color: primary
            .and_then(|style| style.fill_color)
            .or_else(|| fallback.and_then(|style| style.fill_color)),
        background_opacity: primary
            .and_then(|style| style.background_opacity)
            .or_else(|| fallback.and_then(|style| style.background_opacity)),
    }
}

fn apply_background_opacity(color: Color, opacity: Option<f64>) -> Color {
    let Some(opacity) = opacity else {
        return color;
    };
    let clamped = opacity.clamp(0.0, 1.0);
    Color {
        a: (color.a * clamped).clamp(0.0, 1.0),
        ..color
    }
}

fn resolve_font_px(style: &RenderStyle, default_px: f64) -> f64 {
    style.font_size.unwrap_or(default_px)
}

fn resolve_font_family<'a>(style: &'a RenderStyle) -> Option<&'a str> {
    style.font_family.as_deref()
}

fn resolve_font_color(style: &RenderStyle) -> Option<Color> {
    style.font_color
}

fn resolve_stroke_color(style: &RenderStyle) -> Color {
    style.stroke_color.unwrap_or(BORDER_COLOR)
}

fn resolve_stroke_width(style: &RenderStyle, default_width: f64) -> f64 {
    style.stroke_width.unwrap_or(default_width)
}

fn resolve_fill_color(style: &RenderStyle, default_color: Option<Color>) -> Option<Color> {
    let base = style.fill_color.or(default_color)?;
    Some(apply_background_opacity(base, style.background_opacity))
}

/// Render parsed SBGNML glyphs and arcs using bbox geometry.
fn render_sbgnml(
    ctx: &CairoContext,
    transform: &Transform,
    glyphs: &[Glyph],
    arcs: &[Arc],
    render_info: &RenderInfo,
    tag_orientations: &HashMap<String, TagOrientation>,
    show_clone_markers: bool,
) -> Result<()> {
    let logical_font_px = compute_default_label_font_px(glyphs, render_info);
    let mut child_map: HashMap<String, Vec<&Glyph>> = HashMap::new();
    for glyph in glyphs {
        if let Some(parent_id) = &glyph.parent_id {
            child_map.entry(parent_id.clone()).or_default().push(glyph);
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
        render_glyph_tree(
            ctx,
            transform,
            glyph,
            &child_map,
            render_info,
            logical_font_px,
            tag_orientations,
            show_clone_markers,
        )?;
    }

    // Render auxiliary glyphs at their absolute bbox positions.
    for glyph in aux_glyphs {
        let bbox = match glyph.bbox {
            Some(bbox) => bbox,
            None => continue,
        };
        let class_name = glyph.class_name.as_str();
        let label = if class_name == "state variable" && glyph.label.trim().is_empty() {
            state_var_label(
                glyph.state_value.as_deref(),
                glyph.state_variable.as_deref(),
            )
        } else {
            glyph.label.clone()
        };
        let style = merged_style(
            render_info.styles.get(&glyph.id),
            render_info.default_style.as_ref(),
        );
        let font_px = resolve_font_px(&style, glyph_font_px(class_name));
        let font_family = resolve_font_family(&style);
        let font_color = resolve_font_color(&style);
        let has_clone = show_clone_markers && glyph.has_clone;
        match class_name {
            "unit of information" => draw_round_rect_bbox(
                ctx,
                transform,
                bbox,
                &label,
                font_px,
                font_family,
                font_color,
                &style,
                has_clone,
            )?,
            "state variable" => draw_stadium_bbox(
                ctx,
                transform,
                bbox,
                &label,
                font_px,
                font_family,
                font_color,
                &style,
                has_clone,
            )?,
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
        let style = merged_style(
            render_info.styles.get(&arc.id),
            render_info.default_style.as_ref(),
        );
        let stroke_color = resolve_stroke_color(&style);
        let stroke_width = resolve_stroke_width(&style, DEFAULT_LINE_WIDTH);
        draw_arc(
            ctx,
            &points_px,
            &arc.class_name,
            arrow_size_px,
            bar_length_px,
            bar_offset_px,
            stroke_color,
            stroke_width,
        )?;
    }
    Ok(())
}

fn render_glyph_tree(
    ctx: &CairoContext,
    transform: &Transform,
    glyph: &Glyph,
    child_map: &HashMap<String, Vec<&Glyph>>,
    render_info: &RenderInfo,
    logical_font_px: Option<f64>,
    tag_orientations: &HashMap<String, TagOrientation>,
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
        "delay" => Some("τ"),
        "omitted process" => Some("\\\\"),
        "uncertain process" => Some("?"),
        _ => None,
    };
    let mut label = label_override.unwrap_or(glyph.label.as_str()).to_string();
    if class_name == "state variable" && label.trim().is_empty() {
        label = state_var_label(
            glyph.state_value.as_deref(),
            glyph.state_variable.as_deref(),
        );
    }
    let style = merged_style(
        render_info.styles.get(&glyph.id),
        render_info.default_style.as_ref(),
    );
    let mut font_px = resolve_font_px(&style, glyph_font_px(class_name));
    if matches!(
        class_name,
        "and" | "or" | "not" | "delay" | "omitted process" | "uncertain process"
    ) {
        if let Some(default_label_px) = logical_font_px {
            font_px = default_label_px;
        }
    }
    let font_family = resolve_font_family(&style);
    let font_color = resolve_font_color(&style);
    let has_clone = show_clone_markers && glyph.has_clone;
    let children = child_map
        .get(&glyph.id)
        .map(|items| items.as_slice())
        .unwrap_or(&[]);
    let has_u_info_bbox = children
        .iter()
        .any(|child| child.class_name == "unit of information" && child.bbox.is_some());
    let has_s_var_bbox = children
        .iter()
        .any(|child| child.class_name == "state variable" && child.bbox.is_some());
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
    let shape_label = if place_label_bottom {
        ""
    } else {
        label.as_str()
    };

    match class_name {
        "phenotype" | "outcome" => draw_hexagon_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            false,
        )?,
        "perturbing agent" => draw_entity_pool_node(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            class_base,
            is_multimer,
            has_clone,
            u_info_label.as_deref(),
            None,
        )?,
        "simple chemical" | "simple chemical multimer" => {
            draw_entity_pool_node(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                font_family,
                font_color,
                &style,
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
                font_family,
                font_color,
                &style,
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
                font_family,
                font_color,
                &style,
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
                font_family,
                font_color,
                &style,
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
                font_family,
                font_color,
                &style,
                class_base,
                is_multimer,
                has_clone,
                u_info_label.as_deref(),
                s_var_label.as_deref(),
            )?;
        }
        "source and sink" | "empty set" => draw_source_sink_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            has_clone,
        )?,
        "compartment" => draw_barrel_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            has_clone,
        )?,
        "tag" => draw_tag_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            tag_orientations
                .get(&glyph.id)
                .copied()
                .unwrap_or(TagOrientation::Left),
            has_clone,
        )?,
        "association" => draw_ellipse_bbox_filled(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            ASSOCIATION_FILL_COLOR,
        )?,
        "dissociation" => draw_double_circle_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
        )?,
        "process" | "omitted process" | "uncertain process" => {
            draw_square_bbox(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                font_family,
                font_color,
                &style,
                false,
            )?;
            if SHOW_PROCESS_DEBUG {
                draw_process_debug_bbox(ctx, transform, bbox)?;
            }
        }
        "unit of information" => draw_round_rect_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            false,
        )?,
        "state variable" => draw_stadium_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            false,
        )?,
        "and" | "or" | "not" | "delay" => {
            draw_circle_bbox(
                ctx,
                transform,
                bbox,
                shape_label,
                font_px,
                font_family,
                font_color,
                &style,
            )?;
            if SHOW_LOGICAL_DEBUG_BBOX {
                draw_logical_debug_bbox(ctx, transform, bbox)?;
            }
        }
        _ => draw_box_bbox(
            ctx,
            transform,
            bbox,
            shape_label,
            font_px,
            font_family,
            font_color,
            &style,
            false,
        )?,
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
        draw_orientation_marker(
            ctx,
            transform,
            bbox,
            orientation,
            connector_len_px,
            resolve_stroke_color(&style),
            resolve_stroke_width(&style, DEFAULT_LINE_WIDTH),
        )?;
    }

    if place_label_bottom {
        let rect = bbox_pixel_rect(transform, bbox);
        draw_text_bottom_centered(ctx, rect, &label, font_px, font_family, font_color)?;
    }

    for child in children.iter().copied() {
        if matches!(
            child.class_name.as_str(),
            "unit of information" | "state variable"
        ) {
            continue;
        }
        render_glyph_tree(
            ctx,
            transform,
            child,
            child_map,
            render_info,
            logical_font_px,
            tag_orientations,
            show_clone_markers,
        )?;
    }

    Ok(())
}

fn draw_box_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let stroke_color = resolve_stroke_color(style);
    let stroke_width = resolve_stroke_width(style, DEFAULT_LINE_WIDTH);
    let fill_color = resolve_fill_color(style, Some(DEFAULT_FILL_COLOR));
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        stroke_width,
        stroke_color,
        fill_color,
        path_rect,
    )
}

fn draw_process_debug_bbox(ctx: &CairoContext, transform: &Transform, bbox: BBox) -> Result<()> {
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
    set_source_color(ctx, BORDER_COLOR);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn draw_logical_debug_bbox(ctx: &CairoContext, transform: &Transform, bbox: BBox) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    ctx.set_source_rgb(1.0, 0.0, 1.0);
    ctx.set_line_width(1.0);
    path_rect(ctx, rect)?;
    ctx.stroke()?;
    set_source_color(ctx, BORDER_COLOR);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn draw_square_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
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
        font_family,
        font_color,
        has_clone,
        resolve_stroke_width(style, DEFAULT_LINE_WIDTH),
        resolve_stroke_color(style),
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)),
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
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    fill: Color,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    path_ellipse(ctx, rect)?;
    ctx.set_line_width(resolve_stroke_width(style, DEFAULT_LINE_WIDTH));
    let fill_color = resolve_fill_color(style, Some(fill)).unwrap_or(fill);
    set_source_color(ctx, fill_color);
    ctx.fill_preserve()?;
    set_source_color(ctx, resolve_stroke_color(style));
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px, font_family, font_color)?;
    Ok(())
}

fn draw_double_circle_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let radius = (rect.width.min(rect.height) / 2.0).max(1.0);
    ctx.new_path();
    ctx.set_line_width(resolve_stroke_width(style, DEFAULT_LINE_WIDTH));
    ctx.arc(
        rect.center.x,
        rect.center.y,
        radius,
        0.0,
        std::f64::consts::TAU,
    );
    let fill_color =
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)).unwrap_or(DEFAULT_FILL_COLOR);
    set_source_color(ctx, fill_color);
    ctx.fill_preserve()?;
    set_source_color(ctx, resolve_stroke_color(style));
    ctx.stroke()?;
    ctx.new_path();
    ctx.arc(
        rect.center.x,
        rect.center.y,
        (radius * 0.6).max(1.0),
        0.0,
        std::f64::consts::TAU,
    );
    set_source_color(ctx, resolve_stroke_color(style));
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px, font_family, font_color)?;
    Ok(())
}

fn draw_round_rect_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        resolve_stroke_width(style, DEFAULT_LINE_WIDTH),
        resolve_stroke_color(style),
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)),
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
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        resolve_stroke_width(style, DEFAULT_LINE_WIDTH),
        resolve_stroke_color(style),
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)),
        path_hexagon,
    )
}

fn draw_source_sink_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    path_ellipse(ctx, rect)?;
    ctx.set_line_width(resolve_stroke_width(style, DEFAULT_LINE_WIDTH));
    let fill_color =
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)).unwrap_or(DEFAULT_FILL_COLOR);
    set_source_color(ctx, fill_color);
    ctx.fill_preserve()?;
    set_source_color(ctx, resolve_stroke_color(style));
    ctx.stroke()?;
    if has_clone {
        draw_clone_marker(ctx, rect, &path_ellipse)?;
        path_ellipse(ctx, rect)?;
        set_source_color(ctx, resolve_stroke_color(style));
        ctx.stroke()?;
    }
    ctx.new_path();
    ctx.move_to(rect.x0, rect.y0 + rect.height);
    ctx.line_to(rect.x0 + rect.width, rect.y0);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px, font_family, font_color)?;
    Ok(())
}

fn draw_barrel_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let border_width = 4.0;
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        resolve_stroke_width(style, border_width),
        resolve_stroke_color(style),
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)),
        path_barrel,
    )
}

fn draw_tag_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    orientation: TagOrientation,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    let notch_left = matches!(orientation, TagOrientation::Left);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        resolve_stroke_width(style, DEFAULT_LINE_WIDTH),
        resolve_stroke_color(style),
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)),
        |ctx, rect| {
            let notch = (rect.height * 0.3).max(2.0);
            path_tag(ctx, rect, notch, notch_left)
        },
    )
}

fn draw_stadium_bbox(
    ctx: &CairoContext,
    transform: &Transform,
    bbox: BBox,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
    has_clone: bool,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    draw_shape_with_clone(
        ctx,
        rect,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        resolve_stroke_width(style, DEFAULT_LINE_WIDTH),
        resolve_stroke_color(style),
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)),
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
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
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
                font_family,
                font_color,
                false,
                resolve_fill_color(style, entity_pool_fill_color(class_name)),
                resolve_stroke_color(style),
                resolve_stroke_width(style, entity_pool_border_width(class_name)),
            )?;
        }
    }

    draw_entity_pool_base_shape(
        ctx,
        rect,
        class_name,
        label,
        font_px,
        font_family,
        font_color,
        has_clone,
        resolve_fill_color(style, entity_pool_fill_color(class_name)),
        resolve_stroke_color(style),
        resolve_stroke_width(style, entity_pool_border_width(class_name)),
    )?;

    draw_entity_pool_aux_items(ctx, rect, class_name, u_info_label, s_var_label)?;
    Ok(())
}

/// Draw the base shape for entity pool nodes without labels or overlays.
fn draw_entity_pool_base_shape(
    ctx: &CairoContext,
    rect: PixelRect,
    class_name: &str,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    has_clone: bool,
    fill_color: Option<Color>,
    stroke_color: Color,
    border_width: f64,
) -> Result<()> {
    match class_name {
        "simple chemical" | "unspecified entity" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            font_family,
            font_color,
            has_clone,
            border_width,
            stroke_color,
            fill_color,
            path_ellipse,
        ),
        "macromolecule" => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            font_family,
            font_color,
            has_clone,
            border_width,
            stroke_color,
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
            font_family,
            font_color,
            has_clone,
            border_width,
            stroke_color,
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
            font_family,
            font_color,
            has_clone,
            border_width,
            stroke_color,
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
            font_family,
            font_color,
            has_clone,
            border_width,
            stroke_color,
            fill_color,
            path_concave_hexagon,
        ),
        _ => draw_shape_with_clone(
            ctx,
            rect,
            label,
            font_px,
            font_family,
            font_color,
            has_clone,
            border_width,
            stroke_color,
            fill_color,
            path_rect,
        ),
    }
}

/// Map entity pool nodes to their fill colors, matching sbgnStyle defaults.
fn entity_pool_fill_color(class_name: &str) -> Option<Color> {
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
                    None,
                    None,
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
    stroke_color: Color,
    stroke_width: f64,
) -> Result<()> {
    let rect = bbox_pixel_rect(transform, bbox);
    set_source_color(ctx, stroke_color);
    ctx.set_line_width(stroke_width);
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
    color: Color,
) -> Result<()> {
    ctx.set_line_width(line_width.max(1.0));
    set_source_color(ctx, color);
    ctx.new_path();
    ctx.move_to(rect.x0, y);
    ctx.line_to(rect.x0 + rect.width, y);
    ctx.stroke()?;
    set_source_color(ctx, BORDER_COLOR);
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

fn port_connector_len_px_for_class(class_name: &str) -> f64 {
    if matches!(class_name, "and" | "or" | "not" | "delay") {
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
    font_family: Option<&str>,
    font_color: Option<Color>,
    padding_px: f64,
) -> Result<()> {
    let text_width = measure_text_width(ctx, label, font_px, font_family);
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
    path_round_rect_impl(
        ctx,
        rect.x0,
        rect.y0,
        rect.width,
        rect.height,
        rect.width * 0.04,
    )?;
    set_source_color(ctx, WHITE_COLOR);
    ctx.fill_preserve()?;
    set_source_color(ctx, BORDER_COLOR);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px, font_family, font_color)?;
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
    font_family: Option<&str>,
    font_color: Option<Color>,
    padding_px: f64,
    min_width: f64,
) -> Result<()> {
    let text_width = measure_text_width(ctx, label, font_px, font_family);
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
    set_source_color(ctx, WHITE_COLOR);
    ctx.fill_preserve()?;
    set_source_color(ctx, BORDER_COLOR);
    ctx.stroke()?;
    draw_text_centered(ctx, rect.center, label, font_px, font_family, font_color)?;
    ctx.set_line_width(DEFAULT_LINE_WIDTH);
    Ok(())
}

/// Measure label width using the current Cairo/Pango context.
fn measure_text_width(
    ctx: &CairoContext,
    text: &str,
    font_px: f64,
    font_family: Option<&str>,
) -> f64 {
    let layout = pangocairo::create_layout(ctx);
    let family = font_family.unwrap_or(FONT_FAMILY);
    let mut font_desc = FontDescription::from_string(family);
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
    font_family: Option<&str>,
    font_color: Option<Color>,
    style: &RenderStyle,
) -> Result<()> {
    let center = transform.map_point(bbox.x + bbox.w / 2.0, bbox.y + bbox.h / 2.0);
    let radius = transform.scale_scalar(bbox.w.min(bbox.h) / 2.0);
    ctx.arc(center.x, center.y, radius, 0.0, std::f64::consts::TAU);
    let fill_color =
        resolve_fill_color(style, Some(DEFAULT_FILL_COLOR)).unwrap_or(DEFAULT_FILL_COLOR);
    set_source_color(ctx, fill_color);
    ctx.fill_preserve()?;
    set_source_color(ctx, resolve_stroke_color(style));
    ctx.stroke()?;
    draw_text_centered(ctx, center, label, font_px, font_family, font_color)?;
    Ok(())
}

fn draw_shape_with_clone<F>(
    ctx: &CairoContext,
    rect: PixelRect,
    label: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
    has_clone: bool,
    line_width: f64,
    stroke_color: Color,
    fill_color: Option<Color>,
    path_fn: F,
) -> Result<()>
where
    F: Fn(&CairoContext, PixelRect) -> Result<()>,
{
    ctx.set_line_width(line_width.max(0.5));
    path_fn(ctx, rect)?;
    if let Some(color) = fill_color {
        set_source_color(ctx, color);
        ctx.fill_preserve()?;
    }
    set_source_color(ctx, stroke_color);
    ctx.stroke()?;
    if has_clone {
        draw_clone_marker(ctx, rect, &path_fn)?;
        path_fn(ctx, rect)?;
        set_source_color(ctx, stroke_color);
        ctx.stroke()?;
    }
    draw_text_centered(ctx, rect.center, label, font_px, font_family, font_color)?;
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
    set_source_color(ctx, CLONE_MARKER_FILL_COLOR);
    ctx.fill_preserve()?;
    set_source_color(ctx, AUX_LINE_COLOR);
    ctx.set_line_width(CLONE_MARKER_STROKE_WIDTH.max(1.0));
    ctx.stroke()?;
    let _ = ctx.restore();
    set_source_color(ctx, BORDER_COLOR);
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
        Point { x: x0, y: y0 + h },
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

fn path_tag(ctx: &CairoContext, rect: PixelRect, notch: f64, notch_left: bool) -> Result<()> {
    let x0 = rect.x0;
    let y0 = rect.y0;
    let x1 = rect.x0 + rect.width;
    let y1 = rect.y0 + rect.height;
    let mid_y = (y0 + y1) / 2.0;
    ctx.new_path();
    if notch_left {
        ctx.move_to(x0 + notch, y0);
        ctx.line_to(x1, y0);
        ctx.line_to(x1, y1);
        ctx.line_to(x0 + notch, y1);
        ctx.line_to(x0, mid_y);
    } else {
        ctx.move_to(x0, y0);
        ctx.line_to(x1 - notch, y0);
        ctx.line_to(x1, mid_y);
        ctx.line_to(x1 - notch, y1);
        ctx.line_to(x0, y1);
    }
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
    ctx.arc(
        right - radius,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
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
    stroke_color: Color,
    stroke_width: f64,
) -> Result<()> {
    if points.len() < 2 {
        return Ok(());
    }

    set_source_color(ctx, stroke_color);
    ctx.set_line_width(stroke_width);
    for pair in points.windows(2) {
        ctx.move_to(pair[0].x, pair[0].y);
        ctx.line_to(pair[1].x, pair[1].y);
        ctx.stroke()?;
    }

    let end = points[points.len() - 1];
    let prev = points[points.len() - 2];

    match class_name {
        "assignment" => draw_open_triangle(ctx, end, prev, arrow_size)?,
        "positive influence" | "stimulation" => {
            draw_open_triangle_opaque(ctx, end, prev, arrow_size, stroke_color)?
        }
        "modulation" | "unknown influence" => {
            draw_open_diamond_opaque(ctx, end, prev, arrow_size, stroke_color)?
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
            draw_open_triangle_opaque(ctx, end, prev, arrow_size, stroke_color)?;
        }
        "catalysis" => draw_filled_circle_tangent(ctx, end, prev, arrow_size * 0.4, stroke_color)?,
        "equivalence arc" => {}
        _ => {}
    }

    Ok(())
}

fn draw_filled_circle(
    ctx: &CairoContext,
    center: Point,
    radius: f64,
    stroke_color: Color,
) -> Result<()> {
    ctx.arc(
        center.x,
        center.y,
        radius.max(1.0),
        0.0,
        std::f64::consts::TAU,
    );
    set_source_color(ctx, WHITE_COLOR);
    ctx.fill_preserve()?;
    set_source_color(ctx, stroke_color);
    ctx.stroke()?;
    Ok(())
}

fn draw_filled_circle_tangent(
    ctx: &CairoContext,
    end: Point,
    prev: Point,
    radius: f64,
    stroke_color: Color,
) -> Result<()> {
    let dx = end.x - prev.x;
    let dy = end.y - prev.y;
    let len = (dx * dx + dy * dy).sqrt();
    if len == 0.0 {
        return draw_filled_circle(ctx, end, radius, stroke_color);
    }
    let ux = dx / len;
    let uy = dy / len;
    let overlap = (radius * CATALYSIS_OVERLAP_RATIO).max(0.0);
    let offset = (radius - overlap).max(0.0);
    let center = Point {
        x: end.x - ux * offset,
        y: end.y - uy * offset,
    };
    draw_filled_circle(ctx, center, radius, stroke_color)
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
    stroke_color: Color,
) -> Result<()> {
    let Some((p1, p2, tip)) = triangle_points(end, prev, size) else {
        return Ok(());
    };
    ctx.move_to(p1.x, p1.y);
    ctx.line_to(p2.x, p2.y);
    ctx.line_to(tip.x, tip.y);
    ctx.close_path();
    set_source_color(ctx, WHITE_COLOR);
    ctx.fill_preserve()?;
    set_source_color(ctx, stroke_color);
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

fn draw_open_diamond_opaque(
    ctx: &CairoContext,
    end: Point,
    prev: Point,
    size: f64,
    stroke_color: Color,
) -> Result<()> {
    let Some((tip, p1, base, p2)) = diamond_points(end, prev, size) else {
        return Ok(());
    };
    ctx.move_to(tip.x, tip.y);
    ctx.line_to(p1.x, p1.y);
    ctx.line_to(base.x, base.y);
    ctx.line_to(p2.x, p2.y);
    ctx.close_path();
    set_source_color(ctx, WHITE_COLOR);
    ctx.fill_preserve()?;
    set_source_color(ctx, stroke_color);
    ctx.stroke()?;
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

fn diamond_points(end: Point, prev: Point, size: f64) -> Option<(Point, Point, Point, Point)> {
    let dx = end.x - prev.x;
    let dy = end.y - prev.y;
    let length = (dx * dx + dy * dy).sqrt();
    if length == 0.0 {
        return None;
    }
    let ux = dx / length;
    let uy = dy / length;
    let center = Point {
        x: end.x - ux * size * 0.5,
        y: end.y - uy * size * 0.5,
    };
    let base = Point {
        x: end.x - ux * size,
        y: end.y - uy * size,
    };
    let perp_x = -uy;
    let perp_y = ux;
    let half_width = size * 0.6;
    let p1 = Point {
        x: center.x + perp_x * half_width,
        y: center.y + perp_y * half_width,
    };
    let p2 = Point {
        x: center.x - perp_x * half_width,
        y: center.y - perp_y * half_width,
    };
    Some((end, p1, base, p2))
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

fn draw_text_centered(
    ctx: &CairoContext,
    center: Point,
    text: &str,
    font_px: f64,
    font_family: Option<&str>,
    font_color: Option<Color>,
) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let layout = pangocairo::create_layout(ctx);
    let family = font_family.unwrap_or(FONT_FAMILY);
    let mut font_desc = FontDescription::from_string(family);
    font_desc.set_absolute_size(font_px * pango::SCALE as f64);
    layout.set_font_description(Some(&font_desc));
    layout.set_alignment(Alignment::Center);
    layout.set_text(text);

    let (width, height) = layout.pixel_size();
    let x = center.x - width as f64 / 2.0;
    let y = center.y - height as f64 / 2.0;
    draw_text_at(ctx, x, y, &layout, font_color)?;
    Ok(())
}

/// Draw text with an outline at the given top-left position.
fn draw_text_at(
    ctx: &CairoContext,
    x: f64,
    y: f64,
    layout: &pango::Layout,
    font_color: Option<Color>,
) -> Result<()> {
    ctx.move_to(x, y);
    pangocairo::layout_path(ctx, layout);
    if TEXT_OUTLINE_WIDTH > 0.0 {
        set_source_color(ctx, WHITE_COLOR);
        ctx.set_line_width(TEXT_OUTLINE_WIDTH);
        ctx.stroke_preserve()?;
    }
    set_source_color(ctx, font_color.unwrap_or(BORDER_COLOR));
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
    font_family: Option<&str>,
    font_color: Option<Color>,
) -> Result<()> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let layout = pangocairo::create_layout(ctx);
    let family = font_family.unwrap_or(FONT_FAMILY);
    let mut font_desc = FontDescription::from_string(family);
    font_desc.set_absolute_size(font_px * pango::SCALE as f64);
    layout.set_font_description(Some(&font_desc));
    layout.set_alignment(Alignment::Center);
    layout.set_text(text);

    let (width, height) = layout.pixel_size();
    let x = rect.center.x - width as f64 / 2.0;
    let y = rect.y0 + rect.height - height as f64 - 2.0;
    draw_text_at(ctx, x, y, &layout, font_color)
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
        "source and sink" | "empty set" => Some((60.0, 60.0)),
        "perturbing agent" => Some((140.0, 60.0)),
        "phenotype" => Some((140.0, 60.0)),
        "process" | "uncertain process" | "omitted process" => Some((25.0, 25.0)),
        "association" | "dissociation" => Some((25.0, 25.0)),
        "compartment" => Some((50.0, 50.0)),
        "tag" => Some((100.0, 65.0)),
        "and" | "or" | "not" | "delay" => Some((40.0, 40.0)),
        _ => None,
    }
}

fn glyph_font_px(class_name: &str) -> f64 {
    match class_name {
        "state variable"
        | "unit of information"
        | "cardinality"
        | "variable value"
        | "terminal" => FONT_SMALL_PX,
        _ => FONT_MAIN_PX,
    }
}

fn parse_sbgn(root: &Element) -> Result<(Vec<Glyph>, Vec<Arc>, Bounds)> {
    let mut arc_nodes = Vec::new();
    collect_descendants_by_name(root, "arc", &mut arc_nodes);

    let mut glyphs = Vec::new();
    let map_node = find_first_descendant(root, "map")
        .ok_or_else(|| anyhow!("SBGN file missing map element"))?;
    for glyph_node in child_elements(map_node).filter(|node| node.name == "glyph") {
        parse_glyph_node(glyph_node, None, &mut glyphs)?;
    }

    let mut arcs = Vec::new();
    for arc in arc_nodes {
        let arc_id = element_attr(arc, "id").unwrap_or_default().to_string();
        let class_name = element_attr(arc, "class").unwrap_or_default().to_string();
        let source = element_attr(arc, "source").map(|value| value.to_string());
        let target = element_attr(arc, "target").map(|value| value.to_string());
        let start = child_elements(arc)
            .find(|node| node.name == "start")
            .ok_or_else(|| anyhow!("Arc missing start"))?;
        let end = child_elements(arc)
            .find(|node| node.name == "end")
            .ok_or_else(|| anyhow!("Arc missing end"))?;

        let mut points = Vec::new();
        points.push(Point {
            x: parse_f64(element_attr(start, "x")).ok_or_else(|| anyhow!("Bad arc start x"))?,
            y: parse_f64(element_attr(start, "y")).ok_or_else(|| anyhow!("Bad arc start y"))?,
        });

        for next in child_elements(arc).filter(|node| node.name == "next") {
            if let (Some(x), Some(y)) = (
                parse_f64(element_attr(next, "x")),
                parse_f64(element_attr(next, "y")),
            ) {
                points.push(Point { x, y });
            }
        }

        points.push(Point {
            x: parse_f64(element_attr(end, "x")).ok_or_else(|| anyhow!("Bad arc end x"))?,
            y: parse_f64(element_attr(end, "y")).ok_or_else(|| anyhow!("Bad arc end y"))?,
        });

        arcs.push(Arc {
            id: arc_id,
            class_name,
            source,
            target,
            points,
        });
    }

    let bounds = compute_bounds(&glyphs, &arcs)?;
    Ok((glyphs, arcs, bounds))
}

fn parse_glyph_node(
    glyph: &Element,
    parent_id: Option<String>,
    glyphs: &mut Vec<Glyph>,
) -> Result<()> {
    // Walk the SBGN XML tree recursively so child glyphs (units, state vars) keep their parent.
    let id = element_attr(glyph, "id").unwrap_or_default().to_string();
    let class_name = element_attr(glyph, "class").unwrap_or_default().to_string();
    let label_node = child_elements(glyph).find(|node| node.name == "label");
    let mut label = label_node
        .and_then(|node| element_attr(node, "text"))
        .unwrap_or("")
        .to_string();
    label = label.replace('\r', "");

    let bbox_node = child_elements(glyph).find(|node| node.name == "bbox");
    let bbox = bbox_node.and_then(|node| parse_bbox(node));

    let ports = child_elements(glyph)
        .filter(|node| node.name == "port")
        .filter_map(|node| {
            let x = parse_f64(element_attr(node, "x"))?;
            let y = parse_f64(element_attr(node, "y"))?;
            Some(Point { x, y })
        })
        .collect();

    let has_clone = child_elements(glyph).any(|node| node.name == "clone");
    let state_node = child_elements(glyph).find(|node| node.name == "state");
    let state_value = state_node
        .and_then(|node| element_attr(node, "value"))
        .map(|value| value.to_string());
    let state_variable = state_node
        .and_then(|node| element_attr(node, "variable"))
        .map(|value| value.to_string());
    let orientation = element_attr(glyph, "orientation").map(|value| value.to_string());

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

    for child in child_elements(glyph).filter(|node| node.name == "glyph") {
        parse_glyph_node(child, Some(glyph_id.clone()), glyphs)?;
    }
    Ok(())
}

fn parse_bbox(node: &Element) -> Option<BBox> {
    Some(BBox {
        x: parse_f64(element_attr(node, "x"))?,
        y: parse_f64(element_attr(node, "y"))?,
        w: parse_f64(element_attr(node, "w"))?,
        h: parse_f64(element_attr(node, "h"))?,
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
        min_x: x_values.iter().copied().fold(f64::INFINITY, f64::min),
        max_x: x_values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        min_y: y_values.iter().copied().fold(f64::INFINITY, f64::min),
        max_y: y_values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
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
