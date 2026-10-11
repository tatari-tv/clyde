//! The dashboard charts, drawn from [`super::super::visuals`].
//!
//! Geometry arrives computed ([`crate::chart`]) and labels arrive formatted (the view model); this
//! module only places them. Bars are HTML whose lengths are CSS custom properties written through
//! [`Page::vars`], so their value text stays readable at phone width; the treemap and calendar are
//! inline SVG with HTML labels laid over them. Every hover target carries `data-tip`, and an SVG
//! target also carries a `<title>` so the tooltip works with JS off.

use super::page::Page;
use crate::chart::{self, calendar, treemap};
use crate::render::visuals::{Calendar, Cascade, DailyStack, Histogram, Pairs, Stack, Treemap};

/// Shown in place of a chart with nothing to draw.
const EMPTY: &str = "No data in this window to draw.";

/// Shown in place of the waterfall when a cache-bearing model has no price.
const WATERFALL_UNPRICED: &str =
    "A model that used the cache has no published price, so the list-price comparison is not drawn.";

fn empty(p: &mut Page, text: &str) {
    p.raw("<p class=\"chart-empty\">").text(text).raw("</p>");
}

pub(super) fn tone_class(tone: usize) -> &'static str {
    match tone {
        0 => " tone-1",
        1 => " tone-2",
        2 => " tone-3",
        3 => " tone-4",
        4 => " tone-5",
        _ => " tone-6",
    }
}

/// Paired %-sessions / %-spend bars with a sort control. Rows past `top_n` fold behind a
/// `<details>` toggle that CSS reads, so the full list is one click away with JS off.
pub(super) fn pairs(p: &mut Page, caption: &str, pairs: Option<&Pairs>, code_labels: bool, top_n: usize) {
    p.raw("<figure class=\"chart pairs-fig\" data-sortable=\"\"");
    p.geom_int("data-top", top_n as i64).raw(">");
    p.raw("<div class=\"fig-head\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Some(pairs) = pairs else {
        p.raw("</div>");
        empty(p, EMPTY);
        p.raw("</figure>\n");
        return;
    };
    if pairs.rows.len() > 1 {
        sort_control(p);
    }
    p.raw("</div>\n<ol class=\"pairs\">\n");
    for (i, row) in pairs.rows.iter().enumerate() {
        p.raw(if i < top_n {
            "<li class=\"pair\""
        } else {
            "<li class=\"pair extra\""
        });
        p.geom_int("data-by-spend", row.rank.spend as i64);
        p.geom_int("data-by-sessions", row.rank.sessions as i64);
        p.geom_int("data-by-per-session", row.rank.per_session as i64);
        p.attr("data-tip", &row.tip).raw(">");
        p.raw(if code_labels {
            "<code class=\"pair-label\">"
        } else {
            "<span class=\"pair-label\">"
        });
        p.text(&row.label);
        p.raw(if code_labels { "</code>" } else { "</span>" });
        p.raw("<div class=\"pair-bars\">");
        bar_track(p, "sessions", row.bars.first, &row.sessions_share, &row.sessions);
        bar_track(p, "spend", row.bars.second, &row.spend_share, &row.spend);
        p.raw("</div></li>\n");
    }
    p.raw("</ol>\n");
    let extra = pairs.rows.len().saturating_sub(top_n);
    if extra > 0 {
        p.raw("<details class=\"more pairs-more\"><summary><span class=\"when-closed\">");
        p.text(&format!(
            "Show all {} rows",
            crate::fmt::format_int(pairs.rows.len() as u64)
        ));
        p.raw("</span><span class=\"when-open\">")
            .text("Show fewer")
            .raw("</span></summary></details>\n");
    }
    p.raw("<div class=\"legend\"><span class=\"key\"><i class=\"swatch sessions\"></i>");
    p.text("% of sessions")
        .raw("</span><span class=\"key\"><i class=\"swatch spend\"></i>");
    p.text("% of spend").raw("</span></div>");
    p.raw("</figure>\n");
}

/// The sort buttons, hidden until `app.js` shows them: without JS they would do nothing.
fn sort_control(p: &mut Page) {
    p.raw("<div class=\"sort\" role=\"group\" hidden=\"\"")
        .attr("aria-label", "Sort rows");
    p.raw("><span class=\"sort-label\">").text("Sort by").raw("</span>");
    for (key, label, pressed) in [
        ("spend", "% spend", true),
        ("sessions", "% sessions", false),
        ("per-session", "Spend per session", false),
    ] {
        p.raw("<button type=\"button\"");
        p.raw(match key {
            "spend" => " data-key=\"by-spend\"",
            "sessions" => " data-key=\"by-sessions\"",
            _ => " data-key=\"by-per-session\"",
        });
        p.raw(if pressed {
            " aria-pressed=\"true\">"
        } else {
            " aria-pressed=\"false\">"
        });
        p.text(label).raw("</button>");
    }
    p.raw("</div>");
}

fn bar_track(p: &mut Page, kind: &'static str, length: i64, share: &str, value: &str) {
    p.raw(match kind {
        "sessions" => "<div class=\"track\"><span class=\"bar sessions\"",
        _ => "<div class=\"track\"><span class=\"bar spend\"",
    });
    p.vars(&[("--w", length)]).raw("></span>");
    p.raw("<span class=\"val\"><b>").text(share).raw("</b> ").text(value);
    p.raw("</span></div>");
}

/// The org -> repo treemap: SVG tiles, HTML labels over the tiles big enough to hold one, and an
/// org legend.
pub(super) fn treemap(p: &mut Page, caption: &str, map: Option<&Treemap>) {
    p.raw("<figure class=\"chart treemap-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Some(map) = map else {
        empty(p, EMPTY);
        p.raw("</figure>\n");
        return;
    };
    p.raw("<div class=\"treemap\"><svg");
    p.geom("viewBox", &format!("0 0 {} {}", treemap::WIDTH, treemap::HEIGHT));
    p.raw(" preserveAspectRatio=\"none\" role=\"img\"")
        .attr("aria-label", caption)
        .raw(">");
    for group in &map.groups {
        p.raw("<g class=\"tm").raw(tone_class(group.tone)).raw("\">");
        for (i, tile) in group.tiles.iter().enumerate() {
            p.raw(match i {
                0 => "<rect class=\"shade-1\"",
                1 => "<rect class=\"shade-2\"",
                2 => "<rect class=\"shade-3\"",
                _ => "<rect class=\"shade-4\"",
            });
            rect_geom(p, tile.rect);
            p.attr("data-tip", &tile.tip)
                .raw("><title>")
                .text(&tile.tip)
                .raw("</title></rect>");
        }
        // The org's own outline, drawn over its repo tiles, so org boundaries read heavier.
        p.raw("<rect class=\"tm-org\"");
        rect_geom(p, group.rect);
        p.raw("/></g>");
    }
    p.raw("</svg><div class=\"tm-labels\" aria-hidden=\"true\">");
    for group in &map.groups {
        for tile in group.tiles.iter().filter(|t| t.rect.labeled()) {
            p.raw(if tile.rect.labeled_narrow() {
                "<span class=\"tm-label\""
            } else {
                "<span class=\"tm-label wide-only\""
            });
            p.vars(&[
                ("--x", tile.rect.x),
                ("--y", tile.rect.y),
                ("--w", tile.rect.w),
                ("--h", tile.rect.h),
            ]);
            p.raw("><b>").text(&tile.label).raw("</b><span>").text(&tile.spend);
            p.raw("</span></span>");
        }
    }
    p.raw("</div></div>\n<ul class=\"legend tm-legend\">");
    for group in &map.groups {
        p.raw("<li><i class=\"swatch")
            .raw(tone_class(group.tone))
            .raw("\"></i><code>");
        p.text(&group.label)
            .raw("</code> <span class=\"figures\">")
            .text(&group.spend);
        p.text(" \u{b7} ").text(&group.share).raw("</span></li>");
    }
    p.raw("</ul></figure>\n");
}

fn rect_geom(p: &mut Page, r: treemap::Rect) {
    p.geom("x", &chart::tenths(r.x)).geom("y", &chart::tenths(r.y));
    p.geom("width", &chart::tenths(r.w)).geom("height", &chart::tenths(r.h));
}

/// One bar split by model spend, with a legend row per model.
pub(super) fn stack(p: &mut Page, caption: &str, stack: Option<&Stack>) {
    p.raw("<figure class=\"chart stack-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Some(stack) = stack else {
        empty(p, EMPTY);
        p.raw("</figure>\n");
        return;
    };
    stack_bar(p, stack);
    p.raw("<ul class=\"legend stack-legend\">");
    for seg in &stack.segments {
        p.raw("<li><i class=\"swatch")
            .raw(tone_class(seg.tone))
            .raw("\"></i><code>");
        p.text(&seg.label)
            .raw("</code> <span class=\"figures\">")
            .text(&seg.spend);
        p.text(" \u{b7} ").text(&seg.share).raw("</span></li>");
    }
    p.raw("</ul></figure>\n");
}

/// The outcome bar alone: the band table beneath it is the legend, so the figure draws none.
pub(super) fn band_bar(p: &mut Page, caption: &str, stack: Option<&Stack>) {
    p.raw("<figure class=\"chart stack-fig band-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    match stack {
        Some(stack) => stack_bar(p, stack),
        None => empty(p, EMPTY),
    }
    p.raw("</figure>\n");
}

fn stack_bar(p: &mut Page, stack: &Stack) {
    p.raw("<div class=\"stackbar\">");
    for seg in &stack.segments {
        p.raw("<span class=\"seg").raw(tone_class(seg.tone)).raw("\"");
        p.vars(&[("--x", seg.segment.x), ("--w", seg.segment.w)]);
        p.attr("data-tip", &seg.tip).raw(">");
        if seg.segment.labeled() {
            p.raw("<span>").text(&seg.share).raw("</span>");
        }
        p.raw("</span>");
    }
    p.raw("</div>");
}

/// List price -> cache savings -> spend.
pub(super) fn waterfall(p: &mut Page, caption: &str, cascade: &Cascade) {
    p.raw("<figure class=\"chart waterfall-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Cascade::Drawn {
        list,
        savings,
        spend,
        steps,
    } = cascade
    else {
        let text = match cascade {
            Cascade::Unpriced => WATERFALL_UNPRICED,
            _ => EMPTY,
        };
        empty(p, text);
        p.raw("</figure>\n");
        return;
    };
    p.raw("<ol class=\"steps\">");
    step(p, "step list", "List price", steps.list, list);
    step(
        p,
        if steps.savings_rise {
            "step premium"
        } else {
            "step savings"
        },
        if steps.savings_rise {
            "Cache premium"
        } else {
            "Cache savings"
        },
        steps.savings,
        savings,
    );
    step(p, "step spend", "Spend", steps.spend, spend);
    p.raw("</ol></figure>\n");
}

fn step(p: &mut Page, class: &'static str, label: &str, geometry: chart::waterfall::Step, value: &str) {
    p.raw("<li class=\"")
        .raw(class)
        .raw("\"><span class=\"step-label\">")
        .text(label);
    p.raw("</span><div class=\"track\"><span class=\"bar\"");
    p.vars(&[("--x", geometry.start), ("--w", geometry.width)])
        .raw("></span>");
    p.raw("<span class=\"val\"><b>")
        .text(value)
        .raw("</b></span></div></li>");
}

/// Sessions per cost bucket: columns, counts above, cost edges below, median and 90th-percentile
/// marks under their columns.
pub(super) fn histogram(p: &mut Page, caption: &str, histogram: Option<&Histogram>) {
    p.raw("<figure class=\"chart histo-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Some(h) = histogram else {
        empty(p, EMPTY);
        p.raw("</figure>\n");
        return;
    };
    let n = h.columns.len() as i64;
    p.raw("<div class=\"histo\"").vars(&[("--n", n * 10)]).raw(">");
    p.raw("<div class=\"cols\">");
    for col in &h.columns {
        p.raw("<div class=\"col\"").attr("data-tip", &col.tip).raw(">");
        p.raw("<span class=\"count\">").text(&col.count).raw("</span>");
        p.raw("<span class=\"colbar\"")
            .vars(&[("--h", col.height)])
            .raw("></span></div>");
    }
    p.raw("</div><div class=\"edges\" aria-hidden=\"true\">");
    for (i, edge) in h.edges.iter().enumerate() {
        p.raw("<span")
            .vars(&[("--i", i as i64 * 10)])
            .raw(">")
            .text(edge)
            .raw("</span>");
    }
    p.raw("</div><div class=\"marks\">");
    for col in &h.columns {
        p.raw("<span>");
        let mark = match (col.median, col.p90) {
            (true, true) => Some("median, 90th pct"),
            (true, false) => Some("median"),
            (false, true) => Some("90th pct"),
            (false, false) => None,
        };
        if let Some(mark) = mark {
            p.text(mark);
        }
        p.raw("</span>");
    }
    p.raw("</div></div>");
    if let Some(note) = &h.unpriced {
        p.raw("<p class=\"caption\">").text(note).raw("</p>");
    }
    p.raw("</figure>\n");
}

/// Daily spend as standing columns split by model, one per day, on the tallest day's scale. Blocks
/// take the Cost Summary bar's model colors; each day's whole column is one hover target carrying
/// the day's total and its per-model split.
pub(super) fn daily_stack(p: &mut Page, caption: &str, daily: Option<&DailyStack>) {
    p.raw("<figure class=\"chart daily-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Some(daily) = daily else {
        empty(p, EMPTY);
        p.raw("</figure>\n");
        return;
    };
    p.raw("<div class=\"plot columns\">");
    super::line::y_axis(p, &daily.y_labels);
    p.raw("<svg");
    p.geom("viewBox", &format!("0 0 {} {}", chart::FULL, chart::FULL));
    p.raw(" preserveAspectRatio=\"none\" role=\"img\"")
        .attr("aria-label", caption)
        .raw("><g class=\"grid\">");
    for y in [0, chart::FULL / 2, chart::FULL] {
        p.raw("<line x1=\"0\"").geom_int("x2", chart::FULL);
        p.geom_int("y1", y).geom_int("y2", y).raw("/>");
    }
    p.raw("</g>");
    for col in &daily.columns {
        p.raw("<g class=\"day\"").attr("data-tip", &col.tip).raw("><title>");
        p.text(&col.tip).raw("</title>");
        p.raw("<rect class=\"hit\"").geom_int("x", col.x).raw(" y=\"0\"");
        p.geom_int("width", col.w).geom_int("height", chart::FULL).raw("/>");
        for (block, tone) in &col.blocks {
            p.raw("<rect class=\"blk").raw(tone_class(*tone)).raw("\"");
            p.geom_int("x", col.x).geom_int("y", block.y);
            p.geom_int("width", col.w).geom_int("height", block.h).raw("/>");
        }
        p.raw("</g>");
    }
    p.raw("</svg>");
    super::line::x_axis(p, &daily.x_labels);
    p.raw("</div><ul class=\"legend stack-legend\">");
    for (model, tone) in &daily.legend {
        p.raw("<li><i class=\"swatch")
            .raw(tone_class(*tone))
            .raw("\"></i><code>");
        p.text(model).raw("</code></li>");
    }
    p.raw("</ul></figure>\n");
}

/// The daily spend calendar: weekday header, SVG cells, a shade legend.
pub(super) fn calendar(p: &mut Page, caption: &str, cal: Option<&Calendar>) {
    p.raw("<figure class=\"chart cal-fig\"><figcaption>")
        .text(caption)
        .raw("</figcaption>");
    let Some(cal) = cal else {
        empty(p, EMPTY);
        p.raw("</figure>\n");
        return;
    };
    p.raw("<div class=\"cal\"><div class=\"cal-days\" aria-hidden=\"true\">");
    for day in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] {
        p.raw("<span>").text(day).raw("</span>");
    }
    p.raw("</div><svg");
    p.geom("viewBox", &format!("0 0 {} {}", calendar::WIDTH, cal.height));
    p.raw(" role=\"img\"").attr("aria-label", caption).raw(">");
    for (cell, tip) in &cal.cells {
        p.raw(match cell.level {
            0 => "<rect class=\"l0\"",
            1 => "<rect class=\"l1\"",
            2 => "<rect class=\"l2\"",
            3 => "<rect class=\"l3\"",
            _ => "<rect class=\"l4\"",
        });
        p.geom_int("x", cell.x).geom_int("y", cell.y);
        p.geom_int("width", calendar::CELL).geom_int("height", calendar::CELL);
        p.raw(" rx=\"2\"")
            .attr("data-tip", tip)
            .raw("><title>")
            .text(tip)
            .raw("</title></rect>");
    }
    p.raw("</svg><div class=\"cal-legend\" aria-hidden=\"true\"><span>")
        .text("Less");
    p.raw("</span><i class=\"l0\"></i><i class=\"l1\"></i><i class=\"l2\"></i><i class=\"l3\"></i><i class=\"l4\"></i><span>");
    p.text("More").raw("</span></div></div></figure>\n");
}

#[cfg(test)]
mod tests;
