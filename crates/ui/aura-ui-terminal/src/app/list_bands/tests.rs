use super::*;
use crate::app::AgentsTab;
use crate::events::{ChannelSummary, UiCommand};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;

/// The pitch of a 14 px font: 18 px cells, list rows 1.3 as tall.
const DESKTOP: ListPitch = ListPitch { cell: 18, tall: 23 };

fn draw(app: &mut App, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(120, height)).expect("backend");
    terminal
        .draw(|frame| crate::render(frame, app, &crate::Theme::default()))
        .expect("draw");
    terminal.backend().buffer().clone()
}

/// The app on CHANNELS with `count` channels, the last one highlighted.
fn many_channels(count: usize) -> App {
    let mut app = App::new();
    let channels = (0..count)
        .map(|n| ChannelSummary {
            id: format!("c{n}"),
            slug: format!("chan{n}"),
            ..ChannelSummary::default()
        })
        .collect();
    app.process_command(UiCommand::SetChannels(channels));
    app.set_agents_tab(AgentsTab::Channels);
    app.move_channel_highlight(isize::try_from(count).unwrap_or(0));
    app
}

#[test]
fn the_default_pitch_leaves_every_list_as_it_was() {
    let mut app = many_channels(40);
    let rows = Rect::new(2, 4, 30, 20);
    assert_eq!(app.list_rows(Rect::new(1, 3, 32, 22), rows), rows);
    draw(&mut app, 40);
    assert!(app.list_bands().is_empty(), "a terminal records no band");
}

#[test]
fn a_taller_pitch_fits_fewer_rows_from_the_top() {
    assert_eq!(DESKTOP.fit(20), 15);
    assert_eq!(DESKTOP.fit(1), 1, "a row is never lost");
    assert_eq!(DESKTOP.fit(0), 0);
    assert_eq!(ListPitch::default().fit(20), 20);

    let mut app = App::new();
    app.set_list_pitch(DESKTOP);
    let panel = Rect::new(1, 3, 32, 22);
    let shrunk = app.list_rows(panel, Rect::new(2, 4, 30, 20));
    assert_eq!(shrunk, Rect::new(2, 4, 30, 15));
    let band = &app.list_bands()[0];
    assert_eq!((band.columns.clone(), band.rows.clone()), (1..33, 4..24));
    assert_eq!(band.shown, 15);
}

#[test]
fn channels_keep_their_highlight_in_the_rows_they_keep() {
    let mut app = many_channels(40);
    app.set_list_pitch(DESKTOP);
    let buffer = draw(&mut app, 40);
    let band = app
        .list_bands()
        .first()
        .cloned()
        .expect("CHANNELS is a band");
    let text = |y: u16| -> String {
        band.columns
            .clone()
            .map(|x| buffer[(x, y)].symbol().to_string())
            .collect()
    };
    let last_shown = band.rows.start + band.shown - 1;
    assert!(text(last_shown).contains("chan39"), "{:?}", text(last_shown));
    for y in band.rows.start + band.shown..band.rows.end {
        let inside = text(y);
        let inside = inside.trim_matches(|c| c == '│' || c == ' ');
        assert!(inside.is_empty(), "the rows under the list are blank: {inside:?}");
    }
    assert_eq!(band.row(band.rows.start).len(), band.columns.len());
}

#[test]
fn what_is_drawn_over_a_band_is_recorded_as_covered() {
    let area = Rect::new(0, 0, 10, 4);
    let mut buffer = Buffer::empty(area);
    let mut band = ListBand {
        columns: 2..8,
        rows: 1..3,
        shown: 1,
        cells: Vec::new(),
        covered: Vec::new(),
    };
    band.capture(&buffer);
    band.cover(&buffer);
    assert!(band.covered(1).next().is_none(), "nothing drawn over it");

    // A menu over columns 4 to 9 of row 2, past the band's right edge.
    for x in 4..10 {
        if let Some(cell) = buffer.cell_mut((x, 2)) {
            cell.set_symbol("m").set_bg(Color::Blue);
        }
    }
    band.cover(&buffer);
    assert_eq!(band.covered(2).collect::<Vec<_>>(), [4..8]);
    assert!(band.is_covered(5, 2));
    assert!(!band.is_covered(3, 2));
    assert!(!band.is_covered(5, 1));
    assert_eq!(band.row(2)[3].symbol(), " ", "the copy keeps what was under");
}
