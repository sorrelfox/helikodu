// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! The track table: a GtkColumnView over the lazy ID model.

use std::cell::Cell;

use gtk::prelude::*;

use crate::model::{IdModel, RowItem};

thread_local! {
    pub static CELLS_CREATED: Cell<u64> = const { Cell::new(0) };
    pub static CELL_BINDS: Cell<u64> = const { Cell::new(0) };
}

#[derive(Debug, Clone, Copy)]
enum Col {
    Number,
    Title,
    Artist,
    Album,
    Year,
    Genre,
    Duration,
    Format,
    Composer,
    Bitrate,
    Plays,
    Rating,
}

impl Col {
    fn spec(self) -> (&'static str, i32, bool) {
        match self {
            Col::Number => ("#", 48, false),
            Col::Title => ("Title", 260, true),
            Col::Artist => ("Artist", 180, true),
            Col::Album => ("Album", 200, true),
            Col::Year => ("Year", 60, false),
            Col::Genre => ("Genre", 110, false),
            Col::Duration => ("Time", 64, false),
            Col::Format => ("Format", 110, false),
            Col::Composer => ("Composer", 160, false),
            Col::Bitrate => ("Bitrate", 90, false),
            Col::Plays => ("Plays", 60, false),
            Col::Rating => ("Rating", 90, false),
        }
    }
}

const COLUMNS: [Col; 12] = [
    Col::Number,
    Col::Title,
    Col::Artist,
    Col::Album,
    Col::Year,
    Col::Genre,
    Col::Duration,
    Col::Format,
    Col::Composer,
    Col::Bitrate,
    Col::Plays,
    Col::Rating,
];

fn draw_stars(cr: &gtk::cairo::Context, h: i32, rating: u8, color: gtk::gdk::RGBA) {
    for i in 0..5u8 {
        let cx = 8.0 + f64::from(i) * 15.0;
        let cy = f64::from(h) / 2.0;
        for k in 0..10 {
            let r = if k % 2 == 0 { 6.0 } else { 2.6 };
            let a = std::f64::consts::PI * (f64::from(k) / 5.0) - std::f64::consts::FRAC_PI_2;
            let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
            if k == 0 {
                cr.move_to(x, y)
            } else {
                cr.line_to(x, y)
            }
        }
        cr.close_path();
        let alpha = if i < rating { 1.0 } else { 0.25 };
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            alpha,
        );
        let _ = cr.fill();
    }
}

fn factory(col: Col) -> gtk::SignalListItemFactory {
    let f = gtk::SignalListItemFactory::new();
    f.connect_setup(move |_, li| {
        let Some(li) = li.downcast_ref::<gtk::ColumnViewCell>() else {
            return;
        };
        CELLS_CREATED.with(|c| c.set(c.get() + 1));
        if let Col::Rating = col {
            let area = gtk::DrawingArea::new();
            area.set_content_width(80);
            area.set_content_height(16);
            li.set_child(Some(&area));
            return;
        }
        let l = gtk::Label::new(None);
        l.set_xalign(0.0);
        l.set_single_line_mode(true);
        l.set_ellipsize(gtk::pango::EllipsizeMode::End);
        match col {
            Col::Number | Col::Year | Col::Duration | Col::Plays | Col::Bitrate => {
                l.add_css_class("hk-numeric");
                l.add_css_class("hk-t3");
            }
            Col::Title => {}
            Col::Format => {
                l.add_css_class("hk-mono");
                l.add_css_class("hk-t3");
            }
            _ => l.add_css_class("hk-t2"),
        }
        li.set_child(Some(&l));
    });
    f.connect_bind(move |_, li| {
        let Some(li) = li.downcast_ref::<gtk::ColumnViewCell>() else {
            return;
        };
        let Some(item) = li.item().and_downcast::<RowItem>() else {
            return;
        };
        CELL_BINDS.with(|c| c.set(c.get() + 1));
        let t = item.track();
        let Some(child) = li.child() else { return };
        if let Col::Number = col {
            // Striping from the item position, set on the row widget
            // (cell -> row). A CSS :nth-child rule would move as rows recycle.
            if let Some(row) = child.parent().and_then(|cell| cell.parent()) {
                if item.position() % 2 == 1 {
                    row.add_css_class("hk-alt");
                } else {
                    row.remove_css_class("hk-alt");
                }
            }
        }
        if let Col::Rating = col {
            if let Ok(area) = child.downcast::<gtk::DrawingArea>() {
                let rating = t.rating;
                area.set_draw_func(move |a, cr, _w, h| draw_stars(cr, h, rating, a.color()));
            }
            return;
        }
        let Ok(l) = child.downcast::<gtk::Label>() else {
            return;
        };
        let text = match col {
            Col::Number => t.number.to_string(),
            Col::Title => t.title.clone(),
            Col::Artist => t.artist.clone(),
            Col::Album => t.album.clone(),
            Col::Year => t.year.to_string(),
            Col::Genre => t.genre.to_owned(),
            Col::Duration => t.duration.clone(),
            Col::Format => t.format.to_owned(),
            Col::Composer => t.composer.clone(),
            Col::Bitrate => t.bitrate.clone(),
            Col::Plays => t.plays.to_string(),
            Col::Rating => String::new(),
        };
        l.set_text(&text);
    });
    f
}

pub fn build(rows: u32, columns: usize) -> gtk::ScrolledWindow {
    // IDs in "query order": a permutation, as a sorted query would return.
    let ids: Vec<u32> = (0..rows)
        .map(|i| (i.wrapping_mul(2_654_435_761)) % rows.max(1))
        .collect();
    let model = IdModel::new(ids);
    let selection = gtk::SingleSelection::new(Some(model));
    let view = gtk::ColumnView::new(Some(selection));
    view.add_css_class("hk-table");
    view.set_reorderable(true);
    let cols: Vec<Col> = if columns >= 12 {
        COLUMNS.to_vec()
    } else {
        // Default eight: #, Title, Artist, Album, Year, Genre, Time, Format.
        COLUMNS[..columns.min(8)].to_vec()
    };
    for col in cols {
        let (title, width, expand) = col.spec();
        let c = gtk::ColumnViewColumn::new(Some(title), Some(factory(col)));
        c.set_fixed_width(width);
        c.set_expand(expand);
        c.set_resizable(true);
        view.append_column(&c);
    }
    let sw = gtk::ScrolledWindow::new();
    sw.set_vexpand(true);
    sw.set_child(Some(&view));
    sw
}
