//! Fixtures shared by the router's tests.

use crate::router::*;
use aura_ui::events::{MessageData, MessageRole};
use aura_ui_terminal::AlertSettings;

pub(super) fn tabs_with(restore: &[&[&str]]) -> (Tabs, mpsc::Receiver<UiEvent>) {
    let (session, events) = mpsc::channel(64);
    let settings = Settings {
        verbose: false,
        alerts: AlertSettings::from_env(),
        measure: crate::text::prose_cells,
    };
    let restore = restore
        .iter()
        .map(|group| group.iter().map(ToString::to_string).collect())
        .collect();
    (Tabs::new(session, settings, restore), events)
}

pub(super) fn tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
    tabs_with(&[])
}

/// `(id, depth)` of each agent; depth 0 heads a project.
pub(super) type Agents<'a> = &'a [(&'a str, usize)];

pub(super) fn list(agents: Agents, active: &str) -> UiCommand {
    UiCommand::SetAgents(
        agents
            .iter()
            .map(|(id, depth)| AgentSummary {
                id: (*id).into(),
                name: id.to_uppercase(),
                depth: *depth,
                is_active: *id == active,
                ..AgentSummary::default()
            })
            .collect(),
    )
}

/// What the session sends at the start.
pub(super) fn start(tabs: &mut Tabs, agents: Agents, active: &str) {
    tabs.apply(list(agents, active));
    tabs.apply(UiCommand::SetActiveAgent(active.into()));
}

/// What the session sends when it switches to `active`.
pub(super) fn switch(tabs: &mut Tabs, agents: Agents, active: &str) {
    tabs.apply(UiCommand::SetActiveAgent(active.into()));
    tabs.apply(list(agents, active));
    tabs.apply(UiCommand::SetActiveAgent(active.into()));
}

pub(super) fn sent(events: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(event) = events.try_recv() {
        out.push(format!("{event:?}"));
    }
    out
}

/// The agent each tab shows.
pub(super) fn shown(tabs: &Tabs) -> Vec<&str> {
    tabs.iter().map(Tab::agent).collect()
}

/// The agents each tab lists.
pub(super) fn held(tabs: &Tabs) -> Vec<Vec<&str>> {
    tabs.iter()
        .map(|tab| tab.tui.agents().iter().map(|a| a.id.as_str()).collect())
        .collect()
}

/// The listed projects, by id.
pub(super) fn projects(ids: &[&str]) -> UiCommand {
    UiCommand::SetProjects(
        ids.iter()
            .map(|id| ProjectSummary {
                id: (*id).into(),
                name: (*id).into(),
                ..ProjectSummary::default()
            })
            .collect(),
    )
}

/// The projects `tab` lists, by id.
pub(super) fn listed(tab: &Tab) -> Vec<String> {
    tab.tui.projects().iter().map(|p| p.id.clone()).collect()
}

pub(super) fn message(text: &str) -> UiCommand {
    UiCommand::ShowMessage(MessageData::new(MessageRole::System, text))
}

pub(super) fn messages(tabs: &Tabs) -> Vec<usize> {
    tabs.iter().map(|tab| tab.tui.messages().len()).collect()
}

pub(super) const ONE: Agents<'static> = &[("a1", 0), ("a1.1", 1), ("p2", 0)];
pub(super) const TWO: Agents<'static> = &[("a1", 0), ("a1.1", 1), ("p2", 0), ("a3", 0)];

/// `+`, then a folder that is no project yet picked in the browser it
/// opens in the tab in front.
pub(super) fn plus(tabs: &mut Tabs) {
    tabs.new_tab();
    assert!(
        tabs.front().tui.folder_picker().is_some(),
        "+ asks for a folder"
    );
    let event = UiEvent::CreateProject {
        folder: "/new/project".into(),
        switch: true,
    };
    tabs.forward(tabs.front_index(), event);
}

/// Two tabs: every project of [`ONE`], and `a3` opened with `+` and
/// in front.
pub(super) fn two_tabs() -> (Tabs, mpsc::Receiver<UiEvent>) {
    let (mut tabs, mut events) = tabs();
    start(&mut tabs, ONE, "a1");
    plus(&mut tabs);
    switch(&mut tabs, TWO, "a3");
    let _ = sent(&mut events);
    (tabs, events)
}
