//! The facts the PLAN view shows under a plan's title: what the plan
//! records of where it came from ([`super::PlanOrigin`]), joined with what
//! the app knows now (the author's channel and project, its merge queue
//! entry, the REPO tab's branch and PRs) so they stay current.
//!
//! Every fact is optional: one with no source is left out, so an old plan
//! shows only its status and progress.

mod status;
mod when;

pub(crate) use status::plan_status;
pub(crate) use when::when;

use super::PlanDoc;
use crate::app::App;
use crate::events::{AgentSummary, ChannelSummary, MergeEntryView};
use crate::repo::{EventKind, EventState, RepoTarget};
use chrono::{DateTime, FixedOffset};

/// How a fact's value is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FactTone {
    /// An ordinary value
    Plain,
    /// An id or a placeholder
    Muted,
    /// What failed
    Error,
}

/// One `label value` pair.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Fact {
    pub(crate) label: &'static str,
    pub(crate) value: String,
    pub(crate) tone: FactTone,
}

impl Fact {
    fn plain(label: &'static str, value: impl Into<String>) -> Self {
        Self {
            label,
            value: value.into(),
            tone: FactTone::Plain,
        }
    }
}

/// The facts of a plan, in two groups the view keeps on rows of their
/// own: who and when, then where it is and how far along.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub(crate) struct PlanFacts {
    pub(crate) who: Vec<Fact>,
    pub(crate) state: Vec<Fact>,
}

impl PlanFacts {
    /// Whether there is nothing to show.
    pub(crate) fn is_empty(&self) -> bool {
        self.who.is_empty() && self.state.is_empty()
    }
}

/// What the app knows of a plan's author.
#[derive(Clone, Copy, Default)]
struct Author<'a> {
    agent: Option<&'a AgentSummary>,
    channel: Option<&'a ChannelSummary>,
    entry: Option<&'a MergeEntryView>,
}

impl App {
    /// The facts of `doc` at `now`, whose time zone the times are shown in.
    pub(crate) fn plan_facts(&self, doc: &PlanDoc, now: &DateTime<FixedOffset>) -> PlanFacts {
        let author = self.plan_author(doc);
        PlanFacts {
            who: who_facts(doc, author, now),
            state: self.state_facts(doc, author),
        }
    }

    /// The author `doc` names, its channel and its newest queue entry.
    fn plan_author(&self, doc: &PlanDoc) -> Author<'_> {
        let Some(id) = doc.origin.author.as_deref() else {
            return Author::default();
        };
        let agent = self
            .agents
            .iter()
            .chain(self.archived.items.iter())
            .find(|agent| agent.id == id);
        let channel = self.channels().iter().find(|channel| {
            channel.coordinator_id == id || channel.members.iter().any(|m| m.id == id)
        });
        let entry = channel.zip(agent).and_then(|(channel, agent)| {
            let queue = self.channel_queue(&channel.id)?;
            queue.entries.iter().find(|entry| {
                entry
                    .agent
                    .as_deref()
                    .is_some_and(|name| Some(name) == agent.code.as_deref() || name == agent.name)
            })
        });
        Author {
            agent,
            channel,
            entry,
        }
    }

    /// Status, machine, branch, PR, project and model.
    fn state_facts(&self, doc: &PlanDoc, author: Author<'_>) -> Vec<Fact> {
        let mut facts = vec![plan_status(doc, author.agent, author.channel, author.entry)];
        if let Some(machine) = &doc.origin.machine {
            facts.push(Fact::plain("machine", machine.clone()));
        }
        let author_id = doc.origin.author.as_deref();
        let repo = self
            .repo_view()
            .filter(|view| author_id == Some(view.agent_id.as_str()));
        let branch = author
            .entry
            .map(|entry| entry.branch.clone())
            .or_else(|| repo.and_then(|view| view.branch.clone()));
        if let Some(branch) = &branch {
            facts.push(Fact::plain("branch", branch.clone()));
        }
        let pr = repo
            .zip(branch.as_deref())
            .and_then(|(view, branch)| pr_of(&view.events, branch));
        facts.extend(pr);
        let project = author.agent.and_then(|agent| agent.project.clone());
        facts.extend(project.map(|project| Fact::plain("project", project)));
        let model = doc.origin.model.clone();
        facts.extend(model.map(|model| Fact::plain("model", model)));
        facts
    }
}

/// Created, updated, by, asked by and assigned.
fn who_facts(doc: &PlanDoc, author: Author<'_>, now: &DateTime<FixedOffset>) -> Vec<Fact> {
    let origin = &doc.origin;
    let mut facts = Vec::new();
    if let Some(created) = origin.created_at {
        facts.push(Fact::plain("created", when(created, now)));
    }
    let updated = origin
        .updated_at
        .filter(|updated| origin.created_at.is_none_or(|created| *updated > created));
    if let Some(updated) = updated {
        facts.push(Fact::plain("updated", when(updated, now)));
    }
    if let Some(id) = &origin.author {
        facts.push(by_fact(id, author.agent));
        facts.push(Fact::plain("asked by", asked_by(author.channel)));
    }
    let assigned = assignees(doc, author.agent);
    if !assigned.is_empty() {
        facts.push(Fact::plain("assigned", assigned.join(", ")));
    }
    facts
}

/// `by RUS199 rust-engineer`, or the id when the author is not listed.
fn by_fact(id: &str, agent: Option<&AgentSummary>) -> Fact {
    match agent {
        Some(agent) => Fact::plain("by", agent_label(agent)),
        None => Fact {
            label: "by",
            value: id.chars().take(8).collect(),
            tone: FactTone::Muted,
        },
    }
}

/// An agent as `CODE name`, or its name when that starts with its code.
fn agent_label(agent: &AgentSummary) -> String {
    match agent.code.as_deref().filter(|code| !code.is_empty()) {
        Some(code) if !agent.name.starts_with(code) => format!("{code} {}", agent.name),
        _ => agent.name.clone(),
    }
}

/// Who asked for a plan by an agent of `channel`: the coordinator for a
/// member, else the user.
fn asked_by(channel: Option<&ChannelSummary>) -> String {
    let Some(channel) = channel else {
        return "you".to_string();
    };
    let coordinator = channel
        .members
        .iter()
        .find(|member| member.id == channel.coordinator_id);
    match coordinator.and_then(|member| member.code.as_deref().or(Some(member.name.as_str()))) {
        Some(name) if !name.is_empty() => format!("{name} in #{}", channel.slug),
        _ => format!("#{}", channel.slug),
    }
}

/// The todos' assignees other than the author, each once, in plan order.
fn assignees(doc: &PlanDoc, author: Option<&AgentSummary>) -> Vec<String> {
    let is_author = |name: &str| {
        author.is_some_and(|agent| agent.code.as_deref() == Some(name) || agent.name == name)
    };
    let mut names: Vec<String> = Vec::new();
    for name in doc.todos.iter().filter_map(|todo| todo.assignee.as_deref()) {
        if !is_author(name) && !names.iter().any(|known| known == name) {
            names.push(name.to_string());
        }
    }
    names
}

/// The newest PR from `branch` among the REPO tab's `events`, as
/// `#67 merged`.
fn pr_of(events: &[crate::repo::RepoEvent], branch: &str) -> Option<Fact> {
    let head = format!("{branch} →");
    let event = events
        .iter()
        .filter(|event| event.kind == EventKind::Pr && event.detail.starts_with(&head))
        .max_by_key(|event| event.at)?;
    let RepoTarget::Pr { number } = event.target else {
        return None;
    };
    let state = match event.state {
        EventState::Open => "open",
        EventState::Draft => "draft",
        EventState::Merged => "merged",
        EventState::Closed => "closed",
        EventState::Done | EventState::Pending => {
            return Some(Fact::plain("PR", format!("#{number}")))
        }
    };
    Some(Fact::plain("PR", format!("#{number} {state}")))
}

#[cfg(test)]
mod tests;
