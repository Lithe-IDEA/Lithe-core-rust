//! Rendering and sanitization for the Markdown dialect shared by every frontend.

use ammonia::Builder;
use comrak::{markdown_to_html, Options};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashSet;

/// SVG elements that only draw. Inline `<svg>` in a README is a picture: shapes,
/// text, gradients, clips, masks, markers and filters survive, while elements that
/// run or embed something else do not (`script`, `foreignObject`, `style`, the
/// animation elements, `feImage`). Names are the case html5ever gives SVG elements.
const SVG_TAGS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "title",
    "desc",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "textPath",
    "image",
    "linearGradient",
    "radialGradient",
    "stop",
    "pattern",
    "clipPath",
    "mask",
    "marker",
    "filter",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
];

/// Geometry and presentation attributes of the SVG elements above. `style` stays
/// out, as it does for HTML; presentation attributes carry the same paint.
const SVG_ATTRIBUTES: &[&str] = &[
    "viewBox",
    "preserveAspectRatio",
    "width",
    "height",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "d",
    "points",
    "pathLength",
    "transform",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-opacity",
    "opacity",
    "color",
    "display",
    "visibility",
    "vector-effect",
    "shape-rendering",
    "text-rendering",
    "paint-order",
    "clip-path",
    "clip-rule",
    "clipPathUnits",
    "mask",
    "maskUnits",
    "maskContentUnits",
    "filter",
    "filterUnits",
    "primitiveUnits",
    "marker-start",
    "marker-mid",
    "marker-end",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "refX",
    "refY",
    "orient",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "dx",
    "dy",
    "rotate",
    "textLength",
    "lengthAdjust",
    "startOffset",
    "text-anchor",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "letter-spacing",
    "word-spacing",
    "text-decoration",
    "in",
    "in2",
    "result",
    "mode",
    "type",
    "values",
    "operator",
    "k1",
    "k2",
    "k3",
    "k4",
    "stdDeviation",
    "flood-color",
    "flood-opacity",
    "radius",
    "tableValues",
    "slope",
    "intercept",
    "amplitude",
    "exponent",
];

/// Keeps a `use` or `textPath` reference inside the document (`#id`): an
/// external reference would load another file's drawing into this one.
fn svg_reference_filter<'u>(
    element: &str,
    attribute: &str,
    value: &'u str,
) -> Option<Cow<'u, str>> {
    if attribute == "href"
        && matches!(element, "use" | "textPath")
        && !value.trim_start().starts_with('#')
    {
        return None;
    }
    Some(Cow::Borrowed(value))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
/// Markdown source to render with the shared dialect.
pub struct MarkdownRenderRequest {
    /// Markdown text to render.
    pub source: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
/// Sanitized HTML safe for embedding in host previews.
pub struct MarkdownRenderResponse {
    /// Sanitized HTML rendering of the source.
    pub html: String,
}

/// The Comrak options of the shared dialect. Rendering and the heading outline
/// parse with the same options, so a heading the preview shows is a heading the
/// outline lists, on the same source line.
pub(crate) fn dialect() -> Options<'static> {
    let mut options = Options::default();
    options.extension.strikethrough = true;
    // The final Ammonia pass is the security boundary. Leaving tagfilter off
    // lets it remove dangerous raw elements together with their contents
    // instead of turning a script body into visible escaped text first.
    options.extension.tagfilter = false;
    options.extension.table = true;
    options.extension.autolink = true;
    options.extension.tasklist = true;
    options.extension.superscript = true;
    options.extension.subscript = true;
    options.extension.footnotes = true;
    options.extension.inline_footnotes = true;
    options.extension.description_lists = true;
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options.extension.alerts = true;
    options.extension.math_dollars = true;
    options.extension.math_latex = true;
    options.extension.math_code = true;
    options.extension.shortcodes = true;
    options.extension.header_id_prefix = Some("lithe-heading-".to_owned());
    options.extension.header_id_prefix_in_href = true;
    options.render.r#unsafe = true;
    options.render.sourcepos = true;
    options
}

/// Renders the shared Markdown dialect used by every Lithe frontend.
///
/// Raw HTML is deliberately enabled in Comrak so useful document elements such
/// as `<details>` survive parsing, then the complete fragment is passed through
/// Ammonia. This keeps formatting HTML while removing executable content,
/// event handlers, embeds, and unsafe URL schemes.
pub fn render(request: MarkdownRenderRequest) -> MarkdownRenderResponse {
    let rendered = markdown_to_html(&request.source, &dialect());
    let mut sanitizer = Builder::default();
    sanitizer
        .add_tags(&["section", "tfoot", "input", "audio", "video", "source"])
        .add_generic_attributes(&["class", "id"])
        .generic_attribute_prefixes(HashSet::from(["data-", "aria-"]))
        .add_tag_attributes("a", &["target"])
        .add_tag_attributes("details", &["open"])
        .add_tag_attributes("img", &["loading"])
        .add_tag_attributes("input", &["type", "checked", "disabled"])
        .add_tag_attributes(
            "audio",
            &["src", "controls", "autoplay", "loop", "muted", "preload"],
        )
        .add_tag_attributes(
            "video",
            &[
                "src",
                "controls",
                "autoplay",
                "loop",
                "muted",
                "poster",
                "preload",
                "width",
                "height",
                "playsinline",
            ],
        )
        .add_tag_attributes("source", &["src", "type", "media"]);

    // README headers commonly align a whole badge row or heading. Keep the
    // presentational attribute without allowing arbitrary inline styles.
    for tag in ["div", "p", "h1", "h2", "h3", "h4", "h5", "h6"] {
        sanitizer.add_tag_attributes(tag, &["align"]);
    }

    // Inline SVG draws; Ammonia checks that an element only switches into the
    // SVG namespace through <svg> and that every href passes the URL schemes.
    sanitizer.add_tags(SVG_TAGS);
    for tag in SVG_TAGS {
        sanitizer.add_tag_attributes(tag, SVG_ATTRIBUTES);
    }
    sanitizer
        .add_tag_attributes("use", &["href"])
        .add_tag_attributes("textPath", &["href"])
        .add_tag_attributes("image", &["href"])
        .attribute_filter(svg_reference_filter);

    MarkdownRenderResponse {
        html: sanitizer.clean(&rendered).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{render, MarkdownRenderRequest};

    fn html(source: &str) -> String {
        render(MarkdownRenderRequest {
            source: source.to_owned(),
        })
        .html
    }

    #[test]
    fn satisfies_shared_render_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/editor/markdown-render-v1.json"
        ))
        .expect("Markdown render fixture should decode");
        for case in fixture["cases"].as_array().expect("fixture cases") {
            let name = case["name"].as_str().expect("case name");
            let output = html(case["source"].as_str().expect("case source"));
            for want in case["contains"].as_array().expect("contains") {
                let want = want.as_str().expect("contains entry");
                assert!(output.contains(want), "{name}: lacks {want:?} in {output}");
            }
            for bad in case["absent"].as_array().expect("absent") {
                let bad = bad.as_str().expect("absent entry");
                assert!(!output.contains(bad), "{name}: has {bad:?} in {output}");
            }
        }
    }

    #[test]
    fn inline_svg_keeps_drawing_and_drops_what_runs_or_embeds() {
        let output = html(
            "<svg viewBox=\"0 0 10 10\"><g transform=\"scale(2)\"><textPath href=\"https://example.com/p\">t</textPath><textPath href=\"#curve\">u</textPath><image href=\"logo.png\" width=\"4\" height=\"4\"/><image href=\"javascript:bad()\"/><filter id=\"f\"><feImage href=\"https://example.com/x.png\"/><feGaussianBlur stdDeviation=\"2\"/></filter><set attributeName=\"fill\" to=\"red\"/><style>rect{fill:red}</style></g></svg>\n",
        );

        assert!(output.contains("<g transform=\"scale(2)\""));
        assert!(output.contains("<textPath href=\"#curve\""));
        assert!(!output.contains("example.com/p"));
        assert!(output.contains("<image href=\"logo.png\""));
        assert!(!output.contains("javascript:"));
        assert!(output.contains("<feGaussianBlur stdDeviation=\"2\""));
        assert!(!output.contains("feImage"));
        assert!(!output.contains("example.com/x.png"));
        assert!(!output.contains("<set"));
        assert!(!output.contains("<style"));
        assert!(!output.contains("fill:red"));
    }

    #[test]
    fn renders_gfm_footnotes_alerts_math_and_shortcodes() {
        let output = html(
            "# Preview\n\n| A | B |\n| - | - |\n| 1 | 2 |\n\n- [x] Ready\n\n~~old~~ :smile: $x^2$ note[^1]\n\n[^1]: Footnote\n\n> [!NOTE]\n> Local only\n",
        );

        assert!(output.contains("<table"));
        assert!(output.contains("type=\"checkbox\""));
        assert!(output.contains("checked=\"\""));
        assert!(output.contains("<del"));
        assert!(output.contains("😄"));
        assert!(output.contains("data-math-style=\"inline\""));
        assert!(output.contains("class=\"footnotes\""));
        assert!(output.contains("markdown-alert-note"));
        assert!(output.contains("id=\"lithe-heading-preview\""));
    }

    #[test]
    fn preserves_diagram_languages_but_does_not_special_case_plantuml() {
        let output = html(
            "```mermaid\ngraph TD; A-->B\n```\n\n```markmap\n# Root\n```\n\n```plantuml\nAlice -> Bob\n```\n",
        );

        assert!(output.contains("class=\"language-mermaid\""));
        assert!(output.contains("class=\"language-markmap\""));
        assert!(output.contains("class=\"language-plantuml\""));
        assert!(output.contains("Alice -&gt; Bob"));
        assert!(!output.contains("plantuml.com"));
        assert!(!output.contains("plantuml-server"));
    }

    #[test]
    fn strips_executable_html_embeds_and_unsafe_urls() {
        let output = html(
            r#"<script>alert('x')</script>
<style>body { display: none }</style>
<iframe src="https://example.com"></iframe>
<object data="file:///tmp/secret"></object>
<img src="x" onerror="alert(1)">
<a href="javascript:alert(1)" onclick="alert(2)">unsafe</a>
<a href="file:///tmp/secret">file</a>"#,
        );

        assert!(!output.contains("<script"));
        assert!(!output.contains("alert('x')"));
        assert!(!output.contains("<style"));
        assert!(!output.contains("display: none"));
        assert!(!output.contains("<iframe"));
        assert!(!output.contains("<object"));
        assert!(!output.contains("onerror"));
        assert!(!output.contains("onclick"));
        assert!(!output.contains("javascript:"));
        assert!(!output.contains("file:///"));
    }

    #[test]
    fn keeps_safe_raw_html_and_hides_front_matter() {
        let output = html(
            "---\ntitle: Hidden\n---\n\n<details open><summary>More</summary><mark>Safe</mark></details>\n",
        );

        assert!(!output.contains("title: Hidden"));
        assert!(output.contains("<details"));
        assert!(output.contains("<summary>More</summary>"));
        assert!(output.contains("<mark>Safe</mark>"));
    }

    #[test]
    fn preserves_rich_content_for_offline_preview() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/editor/markdown-rich-content-v1.json"
        ))
        .expect("rich Markdown fixture should decode");
        let output = html(fixture["source"].as_str().expect("fixture source"));

        assert!(output.contains("H<sub ") && output.contains(">2</sub>O"));
        assert!(output.contains("x<sup ") && output.contains(">2</sup>"));
        assert!(output.contains(">removed</del>"));
        assert!(output.contains(">H~2~O x^2^</code>"));
        assert_eq!(output.matches("data-math-style=\"inline\"").count(), 2);
        assert_eq!(output.matches("data-math-style=\"display\"").count(), 1);
        assert!(output.contains("\\ce{2 H2 + O2 -&gt; 2 H2O}"));
        assert!(output.contains("\\pu{123 kJ}"));
        assert!(output.contains("language-mermaid"));
        assert!(output.contains("class=\"footnotes\""));
    }

    #[test]
    fn preserves_html_alignment_and_inline_badges_without_allowing_styles() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../fixtures/editor/markdown-html-v1.json"
        ))
        .expect("Markdown HTML fixture should decode");
        let output = html(fixture["source"].as_str().expect("fixture source"));

        assert!(output.contains("<div align=\"center\""));
        assert!(output.contains("<h1 align=\"right\""));
        assert!(output.contains("<p align=\"left\""));
        let header = output.split("</div>").next().expect("HTML header");
        assert_eq!(header.matches("<img ").count(), 3);
        assert_eq!(header.matches("<p ").count(), 3);
        assert_eq!(output.matches("<img ").count(), 4);
        assert_eq!(output.matches("<br").count(), 1);
        assert!(output.contains("width=\"80\""));
        assert!(output.contains("height=\"20\""));
        assert!(!output.contains("style="));
        assert!(!output.contains("onclick"));
        assert!(!output.contains("javascript:"));
        assert!(!output.contains("<script"));
        assert!(!output.contains("<style"));
    }
}
