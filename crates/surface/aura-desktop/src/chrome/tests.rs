use super::*;
use aura_ui_terminal::{ChatTab, RecordsTab, SidePanelTab};
use ratatui::style::Color as TermColor;

fn pane(which: Pane, area: Rect, tabs: Vec<PaneTab>) -> PaneChrome {
    PaneChrome {
        pane: which,
        area,
        bar: Rect::new(area.x, area.y, area.width, 2),
        active: tabs.first().copied(),
        tabs,
        icon_tab: None,
        title: None,
        info: None,
        info_short: None,
        info_color: TermColor::Reset,
        closable: false,
        expand: None,
        scope: None,
        focused: false,
    }
}

fn chat_tabs() -> Vec<PaneTab> {
    vec![
        PaneTab::Chat(ChatTab::Chat),
        PaneTab::Chat(ChatTab::Terminal),
    ]
}

fn model(panes: Vec<PaneChrome>) -> Model {
    Model {
        panes,
        covered: false,
        side_toggle: None,
        origin: Point::new(6.0, 6.0),
        metrics: Metrics::new(14.0),
        palette: Palette::from_theme(&aura_ui_terminal::Theme::by_name("cyber")),
    }
}

fn four_panes() -> Model {
    let mut records = pane(Pane::Records, Rect::new(100, 0, 40, 20), Vec::new());
    records.title = Some("KERNEL".into());
    records.info = Some("12 records".into());
    model(vec![
        pane(
            Pane::Agents,
            Rect::new(0, 0, 30, 40),
            vec![PaneTab::Side(SidePanelTab::Trace)],
        ),
        pane(Pane::Chat, Rect::new(30, 0, 70, 40), chat_tabs()),
        records,
        pane(
            Pane::Side,
            Rect::new(100, 20, 40, 20),
            vec![PaneTab::Side(SidePanelTab::Trace)],
        ),
    ])
}

#[test]
fn tabs_are_packed_left_and_as_wide_as_their_labels() {
    let layout = Layout::of(&four_panes());
    let chat = &layout.bars[1];
    assert_eq!(chat.pane, Pane::Chat);
    let [first, second] = [&chat.boxes[0], &chat.boxes[1]];
    assert!(first.rect.x > chat.rect.x && first.rect.x < chat.rect.x + 12.0);
    assert!(second.rect.x > first.rect.x + first.rect.width);
    assert!(second.rect.x < first.rect.x + first.rect.width + 6.0);
    let widths = [first.rect.width, second.rect.width];
    assert!(widths[1] > widths[0], "TERMINAL is longer than CHAT");
    assert!(first.rect.y > chat.rect.y);
    assert!(first.rect.y + first.rect.height < chat.rect.y + chat.rect.height);
}

#[test]
fn untabbed_panels_show_a_title_box_and_their_info() {
    let layout = Layout::of(&four_panes());
    let records = &layout.bars[2];
    assert_eq!(records.boxes.len(), 1);
    assert_eq!(records.boxes[0].text, "KERNEL");
    assert_eq!(records.boxes[0].kind, Kind::Title);
    let info = records.info.as_ref().expect("room for the info");
    assert_eq!(info.text, "12 records");
    assert!(info.end.x <= records.rect.x + records.rect.width);
}

#[test]
fn the_kernel_bar_has_its_tabs_and_keeps_its_count() {
    let tabs = vec![
        PaneTab::Records(RecordsTab::Kernel),
        PaneTab::Records(RecordsTab::Members),
        PaneTab::Records(RecordsTab::Policies),
        PaneTab::Records(RecordsTab::Inbox),
    ];
    let mut records = pane(Pane::Records, Rect::new(100, 0, 50, 20), tabs);
    records.active = Some(PaneTab::Records(RecordsTab::Members));
    records.info = Some("3 AGENTS".into());
    let layout = Layout::of(&model(vec![records]));
    let bar = &layout.bars[0];
    let kinds: Vec<Kind> = bar.boxes.iter().map(|boxed| boxed.kind.clone()).collect();
    assert_eq!(
        kinds,
        [
            Kind::Tab(PaneTab::Records(RecordsTab::Kernel)),
            Kind::Tab(PaneTab::Records(RecordsTab::Members)),
            Kind::Tab(PaneTab::Records(RecordsTab::Policies)),
            Kind::Tab(PaneTab::Records(RecordsTab::Inbox)),
        ]
    );
    assert_eq!(bar.boxes[1].text, "MEMBERS");
    assert_eq!(bar.boxes[2].text, "POLICIES");
    assert_eq!(bar.boxes[3].text, "INBOX");
    let info = bar.info.as_ref().expect("room for the count");
    assert_eq!(info.text, "3 AGENTS");
    assert!(info.end.x > bar.boxes[3].rect.x + bar.boxes[3].rect.width);
}

#[test]
fn narrow_bars_fall_back_to_short_labels_and_drop_what_still_does_not_fit() {
    let mut narrow = pane(Pane::Chat, Rect::new(0, 0, 14, 20), chat_tabs());
    narrow.info = Some("a long info text".into());
    narrow.info_short = Some("info".into());
    let layout = Layout::of(&model(vec![narrow]));
    let bar = &layout.bars[0];
    let labels: Vec<&str> = bar.boxes.iter().map(|b| b.text.as_str()).collect();
    assert_eq!(
        labels,
        [ChatTab::Chat.short_label(), ChatTab::Terminal.short_label()]
    );
    assert!(bar.info.is_none(), "{:?}", bar.info);
}

#[test]
fn the_pointer_hits_tabs_bars_and_dividers() {
    let mut chrome = Chrome::default();
    chrome.update(four_panes(), None);
    let terminal = PaneTab::Chat(ChatTab::Terminal);
    let at = chrome.spot(terminal).expect("drawn");
    assert_eq!(chrome.layout.hit(at), Some(Hit::Tab(terminal)));
    let bar = &chrome.layout.bars[1].rect;
    let empty = Point::new(bar.x + bar.width - 20.0, bar.center_y());
    assert_eq!(chrome.layout.hit(empty), Some(Hit::Bar(Pane::Chat)));
    for divider in [Divider::Agents, Divider::Right, Divider::Records] {
        let at = chrome.band(divider).expect("between two panels");
        assert_eq!(chrome.layout.hit(at), Some(Hit::Divider(divider)));
    }
    // The borders the agents and chat panels draw their scrollbars on
    // are theirs, not the dividers'.
    let cell = chrome.model.as_ref().expect("shown").metrics.cell;
    let on_cell = |col: f32, row: f32| Point::new(6.0 + col * cell.width, 6.0 + row * cell.height);
    assert_eq!(
        chrome.layout.hit(on_cell(99.5, 10.5)),
        None,
        "chat's border"
    );
    assert_eq!(
        chrome.layout.hit(on_cell(29.5, 10.5)),
        None,
        "agents' border"
    );
    let right = chrome.layout.hit(on_cell(100.5, 10.5));
    assert_eq!(right, Some(Hit::Divider(Divider::Right)));
    chrome.point(Some(at));
    assert_eq!(chrome.hover(), Some(Hit::Tab(terminal)));
}

#[test]
fn closable_bars_end_in_a_close_box_left_of_the_info() {
    let mut model = four_panes();
    let chat = &mut model.panes[1];
    chat.tabs.clear();
    chat.title = Some("RECORD #60".into());
    chat.info = Some("POLICY".into());
    chat.closable = true;
    let layout = Layout::of(&model);
    let bar = &layout.bars[1];
    let close = bar.close.expect("room for the close box");
    assert!(close.x + close.width <= bar.rect.x + bar.rect.width);
    let info = bar.info.as_ref().expect("room for the info");
    assert!(info.end.x < close.x);
    let middle = Point::new(close.center_x(), close.center_y());
    assert_eq!(layout.hit(middle), Some(Hit::Close(Pane::Chat)));
    assert!(Layout::of(&four_panes()).bars[1].close.is_none());
}

#[test]
fn expandable_bars_end_in_an_expand_box_left_of_any_close_box() {
    let mut model = four_panes();
    model.panes[1].expand = Some(false);
    let layout = Layout::of(&model);
    let bar = &layout.bars[1];
    let (expand, expanded) = bar.expand.expect("room for the expand box");
    assert!(!expanded);
    assert!(expand.x + expand.width <= bar.rect.x + bar.rect.width);
    let last = bar.boxes.last().expect("the tabs are drawn");
    assert!(last.rect.x + last.rect.width < expand.x);
    let middle = Point::new(expand.center_x(), expand.center_y());
    assert_eq!(layout.hit(middle), Some(Hit::Expand(Pane::Chat)));
    assert!(Layout::of(&four_panes()).bars[1].expand.is_none());

    let chat = &mut model.panes[1];
    chat.tabs.clear();
    chat.title = Some("RECORD #60".into());
    chat.closable = true;
    chat.expand = Some(true);
    let layout = Layout::of(&model);
    let bar = &layout.bars[1];
    let close = bar.close.expect("room for the close box");
    let (expand, expanded) = bar.expand.expect("room for the expand box");
    assert!(expanded);
    assert!(expand.x + expand.width < close.x);
    let middle = Point::new(expand.center_x(), expand.center_y());
    assert_eq!(layout.hit(middle), Some(Hit::Expand(Pane::Chat)));
}

#[test]
fn scoped_bars_end_in_a_star_box_after_their_tabs() {
    let mut model = four_panes();
    model.panes[3].scope = Some(true);
    let layout = Layout::of(&model);
    let bar = &layout.bars[3];
    assert_eq!(bar.pane, Pane::Side);
    let (scope, on) = bar.scope.expect("room for the * box");
    assert!(on);
    assert!(scope.width >= scope.height, "at least square: {scope:?}");
    assert!(scope.x + scope.width <= bar.rect.x + bar.rect.width);
    let last = bar.boxes.last().expect("the tabs are drawn");
    assert!(last.rect.x + last.rect.width < scope.x);
    let middle = Point::new(scope.center_x(), scope.center_y());
    assert_eq!(layout.hit(middle), Some(Hit::Scope(Pane::Side)));
    assert!(Layout::of(&four_panes()).bars[3].scope.is_none());
}

#[test]
fn the_side_bar_ends_in_a_star_box_then_an_expand_box() {
    let mut model = four_panes();
    model.panes[3].scope = Some(false);
    model.panes[3].expand = Some(false);
    let layout = Layout::of(&model);
    let bar = &layout.bars[3];
    let (scope, on) = bar.scope.expect("room for the * box");
    let (expand, expanded) = bar.expand.expect("room for the expand box");
    assert!(!on && !expanded);
    assert!(scope.x + scope.width < expand.x, "{scope:?} {expand:?}");
    assert!(expand.x + expand.width <= bar.rect.x + bar.rect.width);
    let last = bar.boxes.last().expect("the tabs are drawn");
    assert!(last.rect.x + last.rect.width < scope.x);
    let hit = |rect: Rectangle| layout.hit(Point::new(rect.center_x(), rect.center_y()));
    assert_eq!(hit(scope), Some(Hit::Scope(Pane::Side)));
    assert_eq!(hit(expand), Some(Hit::Expand(Pane::Side)));
}

#[test]
fn hosts_icon_is_a_square_tab_box_left_of_the_star_box() {
    let host = PaneTab::Side(SidePanelTab::Host);
    let mut model = four_panes();
    model.panes[3].scope = Some(false);
    model.panes[3].expand = Some(false);
    model.panes[3].icon_tab = Some(host);
    let layout = Layout::of(&model);
    let bar = &layout.bars[3];
    let (icon, tab) = bar.icon_tab.expect("room for the icon box");
    assert_eq!(tab, host);
    assert!(
        (icon.width - icon.height).abs() < f32::EPSILON,
        "square: {icon:?}"
    );
    let (scope, _) = bar.scope.expect("room for the * box");
    assert!(icon.x + icon.width < scope.x, "{icon:?} {scope:?}");
    let last = bar.boxes.last().expect("the tabs are drawn");
    assert!(last.rect.x + last.rect.width < icon.x);
    assert!(
        bar.boxes.iter().all(|boxed| boxed.kind != Kind::Tab(host)),
        "it has no label's box"
    );
    let middle = Point::new(icon.center_x(), icon.center_y());
    assert_eq!(layout.hit(middle), Some(Hit::Tab(host)));

    let mut chrome = Chrome::default();
    chrome.update(model, None);
    assert_eq!(chrome.spot(host), Some(middle));
}

#[test]
fn dragging_a_divider_moves_its_share() {
    let mut chrome = Chrome::default();
    let model = four_panes();
    let cell = model.metrics.cell;
    let (x, y) = (model.origin.x, model.origin.y);
    chrome.update(model, None);
    let splits = PaneSplits::default();

    let to = |col: f32, row: f32| Point::new(x + col * cell.width, y + row * cell.height);
    // The bands' middles are on the borders beside the chat, half a
    // cell past the edges they move.
    let moved = chrome.drag(Divider::Agents, to(14.5, 5.0), splits);
    assert!((moved.agents.expect("set") - 0.1).abs() < 1e-4, "{moved:?}");
    assert_eq!(moved.right, None);

    let moved = chrome.drag(Divider::Right, to(126.5, 5.0), splits);
    assert!((moved.right.expect("set") - 0.1).abs() < 1e-4, "{moved:?}");

    let moved = chrome.drag(Divider::Records, to(110.0, 10.0), splits);
    assert!(
        (moved.records.expect("set") - 0.25).abs() < 1e-4,
        "{moved:?}"
    );

    let past = chrome.drag(Divider::Agents, to(-50.0, 5.0), splits);
    assert_eq!(past.agents, Some(0.0));
}

#[test]
fn covered_bars_are_left_to_what_covers_them() {
    let Model { panes, palette, .. } = four_panes();
    let at = (Point::default(), Metrics::new(14.0), palette);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 140, 40));
    let model = Model::new(&panes, None, &buffer, at);
    assert_eq!(model.panes.len(), 4);
    assert!(!model.covered);
    assert_eq!(Layout::of(&model).bands.len(), 3);

    buffer.set_string(50, 1, "Approve?", ratatui::style::Style::default());
    let model = Model::new(&panes, None, &buffer, at);
    let left: Vec<Pane> = model.panes.iter().map(|p| p.pane).collect();
    assert_eq!(left, [Pane::Agents, Pane::Records, Pane::Side]);
    assert!(model.covered);
    assert!(Layout::of(&model).bands.is_empty());
}

#[test]
fn a_dialogs_backdrop_covers_the_bars_and_the_hamburger() {
    let Model { panes, palette, .. } = four_panes();
    let at = (Point::default(), Metrics::new(14.0), palette);
    let mut buffer = Buffer::empty(Rect::new(0, 0, 140, 40));
    let toggle = Some(Rect::new(138, 0, 2, 1));
    assert!(Model::new(&panes, toggle, &buffer, at)
        .side_toggle
        .is_some());

    // The backdrop leaves every cell blank, but dark.
    let dark = ratatui::style::Style::default().bg(ratatui::style::Color::Rgb(10, 10, 10));
    buffer.set_style(buffer.area, dark);
    let model = Model::new(&panes, toggle, &buffer, at);
    assert!(model.panes.is_empty(), "{:?}", model.panes);
    assert!(model.covered);
    assert!(model.side_toggle.is_none());
    assert!(Layout::of(&model).bands.is_empty());
}
