//! Files of a cloud agent's machine, which the Sandbox panel lists when
//! it shows the machine ([`SandboxTree::is_remote`]): directories and
//! files are asked of the app ([`UiEvent::ListSandbox`],
//! [`UiEvent::ReadSandboxFile`]) and shown when its answer comes. Nothing
//! is asked while the machine sleeps, so looking never wakes it.
//!
//! [`SandboxTree::is_remote`]: super::sandbox::SandboxTree::is_remote

use super::file_view::{FileBody, FileView};
use super::plan_doc::{PlanDoc, TodoFocus};
use super::sandbox::RemoteEntry;
use super::{App, DetailView, NotificationType, SidePanelTab};
use crate::events::UiEvent;
use std::path::{Path, PathBuf};

/// A file asked of the machine and not read yet.
#[derive(Debug)]
pub(crate) struct PendingFile {
    path: PathBuf,
    /// Opens as a plan document, at this todo when one is given
    plan: Option<Option<OwnedFocus>>,
}

/// A [`TodoFocus`] kept until its plan is read.
#[derive(Debug)]
enum OwnedFocus {
    Id(String),
    Nth {
        index: usize,
        count: usize,
        content: String,
    },
}

impl OwnedFocus {
    fn of(focus: TodoFocus<'_>) -> Self {
        match focus {
            TodoFocus::Id(id) => Self::Id(id.to_string()),
            TodoFocus::Nth {
                index,
                count,
                content,
            } => Self::Nth {
                index,
                count,
                content: content.to_string(),
            },
        }
    }

    fn focus(&self) -> TodoFocus<'_> {
        match self {
            Self::Id(id) => TodoFocus::Id(id),
            Self::Nth {
                index,
                count,
                content,
            } => TodoFocus::Nth {
                index: *index,
                count: *count,
                content,
            },
        }
    }
}

impl App {
    /// Ask for the listings a Sandbox tree of the machine still needs,
    /// while the tab shows and the machine is awake.
    pub(super) fn ask_sandbox_listings(&mut self) {
        let shown = self.panels.sandbox_visible && self.side.tab == SidePanelTab::Sandbox;
        if !self.sandbox.is_remote() || !shown || self.machine_asleep() {
            return;
        }
        let Some(tx) = self.link.event_tx.clone() else {
            return;
        };
        for dir in self.sandbox.wanted() {
            if tx.try_send(UiEvent::ListSandbox(dir.clone())).is_ok() {
                self.sandbox.asked(dir);
            }
        }
    }

    /// Show the Sandbox tree the machine's workspace at `root`; one shown
    /// already is listed again.
    pub(super) fn set_sandbox_remote(&mut self, root: PathBuf) {
        if self.sandbox.is_remote() && self.sandbox.root() == Some(root.as_path()) {
            self.ticks.sandbox_stale = true;
            return;
        }
        self.sandbox.set_remote_root(root);
        self.side.sandbox_scroll = 0;
    }

    /// Show what the machine's directory `dir` holds.
    pub(super) fn set_sandbox_listing(
        &mut self,
        dir: PathBuf,
        entries: Result<Vec<RemoteEntry>, String>,
    ) {
        match entries {
            Ok(entries) => self.sandbox.set_listing(dir, entries),
            Err(reason) => self.sandbox.listing_failed(dir, reason),
        }
    }

    /// Show `file` over the chat panel: read here, or asked of the machine
    /// when the Sandbox panel shows one.
    pub(super) fn open_file(&mut self, file: &Path) {
        if self.sandbox.is_remote() {
            self.read_remote_file(file, None);
            return;
        }
        let view = FileView::load(file, self.sandbox.root());
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Ask the machine for `file`, showing that it is read until it comes:
    /// as a plan document, at its todo when given, when `plan` is set.
    pub(super) fn read_remote_file(&mut self, file: &Path, plan: Option<Option<TodoFocus<'_>>>) {
        if self.machine_asleep() {
            self.notifications.current = Some((
                "The machine is asleep; its files can be read once it wakes".into(),
                NotificationType::Warning,
            ));
            return;
        }
        let asked = self.link.event_tx.as_ref().is_some_and(|tx| {
            tx.try_send(UiEvent::ReadSandboxFile(file.to_path_buf()))
                .is_ok()
        });
        if !asked {
            return;
        }
        self.pending.file = Some(PendingFile {
            path: file.to_path_buf(),
            plan: plan.map(|focus| focus.map(OwnedFocus::of)),
        });
        let view = FileView::reading(file, self.sandbox.root());
        self.open_detail(DetailView::File(Box::new(view)));
    }

    /// Show the machine's `path` as read, when the view waiting for it is
    /// still open. It takes that view's place, keeping the way back to
    /// the graph it was opened from.
    pub(super) fn show_sandbox_file(&mut self, path: &Path, text: Result<String, String>) {
        let Some(pending) = self.pending.file.take_if(|pending| pending.path == path) else {
            return;
        };
        let waiting = matches!(
            &self.detail.view,
            Some(DetailView::File(view)) if view.path == path && view.is_reading()
        );
        if !waiting {
            return;
        }
        let view = FileView::read(path, self.sandbox.root(), text);
        if let (Some(focus), FileBody::Text(text)) = (&pending.plan, &view.body) {
            let mut doc = PlanDoc::parse(text, path, &view.title);
            if let Some(focus) = focus {
                doc.focus_on(focus.focus());
            }
            self.detail.view = Some(DetailView::Plan(Box::new(doc)));
            return;
        }
        self.detail.view = Some(DetailView::File(Box::new(view)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::UiCommand;
    use crate::host::{PlacementState, PlacementView};
    use tokio::sync::mpsc;

    const PLAN: &str = "---\nname: Remote plan\noverview: Read on the machine.\ntodos:\n  \
        - id: a\n    content: First\n    status: completed\n  \
        - id: b\n    content: Second\n    status: pending\n---\n\nBody.\n";

    fn app() -> (App, mpsc::Receiver<UiEvent>) {
        let (tx, rx) = mpsc::channel(16);
        let mut app = App::new();
        app.link.event_tx = Some(tx);
        app.active_agent_id = "aa".into();
        app.side.tab = SidePanelTab::Sandbox;
        app.process_command(UiCommand::SetSandboxRemote("/workspace".into()));
        (app, rx)
    }

    fn placed(app: &mut App, asleep: bool) {
        app.process_command(UiCommand::SetPlacement {
            agent_id: "aa".into(),
            state: PlacementState {
                placement: PlacementView::Cloud,
                asleep,
                ..PlacementState::default()
            },
        });
    }

    /// What the app was asked to read, in order.
    fn asked(rx: &mut mpsc::Receiver<UiEvent>) -> Vec<String> {
        let mut asked = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                UiEvent::ListSandbox(dir) => asked.push(format!("list {}", dir.display())),
                UiEvent::ReadSandboxFile(file) => asked.push(format!("read {}", file.display())),
                _ => {}
            }
        }
        asked
    }

    fn listed(app: &mut App, dir: &str, entries: &[(&str, bool)]) {
        let entries = entries
            .iter()
            .map(|&(name, is_dir)| RemoteEntry {
                name: name.into(),
                is_dir,
            })
            .collect();
        app.process_command(UiCommand::SandboxListing {
            dir: dir.into(),
            entries: Ok(entries),
        });
    }

    fn row_names(app: &App) -> Vec<&str> {
        app.sandbox
            .rows()
            .iter()
            .map(|row| row.name.as_str())
            .collect()
    }

    #[test]
    fn the_machine_s_tree_is_asked_for_once_and_never_while_it_sleeps() {
        let (mut app, mut rx) = app();
        placed(&mut app, true);
        app.ask_sandbox_listings();
        assert!(asked(&mut rx).is_empty(), "a sleeping machine is not read");

        placed(&mut app, false);
        app.tick();
        assert_eq!(asked(&mut rx), ["list /workspace"]);
        app.ask_sandbox_listings();
        assert!(asked(&mut rx).is_empty(), "asked once");

        listed(
            &mut app,
            "/workspace",
            &[("src", true), ("README.md", false)],
        );
        assert_eq!(row_names(&app), ["src", "README.md"]);
        app.sandbox.select(0);
        app.activate_selected_sandbox_row();
        app.ask_sandbox_listings();
        assert_eq!(asked(&mut rx), ["list /workspace/src"]);
    }

    #[test]
    fn a_file_of_the_machine_shows_once_read_in_the_view_waiting_for_it() {
        let (mut app, mut rx) = app();
        listed(&mut app, "/workspace", &[("main.rs", false)]);
        app.activate_selected_sandbox_row();
        assert_eq!(asked(&mut rx), ["read /workspace/main.rs"]);
        assert!(matches!(
            app.detail_view(),
            Some(DetailView::File(view)) if view.title == "main.rs" && view.is_reading()
        ));

        app.process_command(UiCommand::SandboxFile {
            path: "/workspace/other.rs".into(),
            text: Ok("other".into()),
        });
        assert!(matches!(
            app.detail_view(),
            Some(DetailView::File(view)) if view.is_reading()
        ));

        app.process_command(UiCommand::SandboxFile {
            path: "/workspace/main.rs".into(),
            text: Ok("fn main() {}\n".into()),
        });
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("a file view");
        };
        assert_eq!(view.body, FileBody::Text("fn main() {}\n".into()));
    }

    #[test]
    fn a_plan_of_the_machine_opens_as_a_document_at_its_todo() {
        let (mut app, mut rx) = app();
        let plan = Path::new("/workspace/.aura/plans/remote_00000001.plan.md");
        app.open_plan_doc(plan, Some(TodoFocus::Id("b")));
        assert_eq!(
            asked(&mut rx),
            ["read /workspace/.aura/plans/remote_00000001.plan.md"]
        );
        app.process_command(UiCommand::SandboxFile {
            path: plan.into(),
            text: Ok(PLAN.into()),
        });
        let Some(DetailView::Plan(doc)) = app.detail_view() else {
            panic!("a plan document");
        };
        assert_eq!(doc.title, "Remote plan");
        assert_eq!(doc.focus, Some(1));
    }

    #[test]
    fn a_closed_view_stays_closed_and_a_sleeping_machine_is_not_asked() {
        let (mut app, mut rx) = app();
        app.open_file(Path::new("/workspace/a.txt"));
        app.close_detail();
        app.process_command(UiCommand::SandboxFile {
            path: "/workspace/a.txt".into(),
            text: Ok("late".into()),
        });
        assert!(app.detail_view().is_none());

        placed(&mut app, true);
        asked(&mut rx);
        app.open_file(Path::new("/workspace/a.txt"));
        assert!(asked(&mut rx).is_empty());
        assert!(app.detail_view().is_none());
        assert!(app.notifications.current.is_some());
    }

    #[test]
    fn a_failed_read_says_why() {
        let (mut app, _rx) = app();
        app.open_file(Path::new("/workspace/gone.txt"));
        app.process_command(UiCommand::SandboxFile {
            path: "/workspace/gone.txt".into(),
            text: Err("not found".into()),
        });
        let Some(DetailView::File(view)) = app.detail_view() else {
            panic!("a file view");
        };
        assert!(!view.is_reading());
        assert!(
            matches!(&view.body, FileBody::Note(note) if note.contains("not found")),
            "{:?}",
            view.body
        );
    }
}
