//! Panel commands: which panels and tabs show.

use crate::{
    app::{AgentsTab, App, BenchMode, ChatTab, PanelFocus},
    events::UiEvent,
};

impl App {
    /// Run `cmd` when it is a panel command; whether it was one.
    pub(super) fn panel_command(&mut self, cmd: &str, arg: &str) -> bool {
        match cmd {
            "kernel" | "record" | "k" | "r" if self.agent_panels_hidden() => {
                self.notify_no_agent_panels();
            }
            "kernel" | "record" | "k" | "r" => self.toggle_record_panel(),
            "cloud" | "local" | "host" => self.handle_cloud_command(cmd, arg),
            "repo" => self.show_repo(arg),
            "members" | "team" => self.show_members(),
            "policies" | "policy" | "perms" | "permissions" => self.show_policies(),
            "inbox" | "mail" => self.show_inbox(),
            "agents" | "swarm" | "sw" => self.toggle_swarm_panel(),
            "files" | "sandbox" | "f" if self.agent_panels_hidden() => {
                self.notify_no_agent_panels();
            }
            "files" | "sandbox" | "f" => self.toggle_sandbox_panel(),
            "expand" | "zoom" | "fullscreen" => self.toggle_chat_expanded(),
            "project" => self.show_active_project_info(),
            "graph" | "g" => self.request_graph(arg),
            "log" | "logs" => self.show_chat_tab(ChatTab::Log),
            "bench" | "benchmarks" => self.bench_command(arg),
            _ => return false,
        }
        true
    }

    /// `/record`: show or hide the Record panel.
    fn toggle_record_panel(&mut self) {
        if self.side_maximized() {
            // Hidden under the side panel, it comes back.
            self.panels.side_expanded = false;
            self.panels.record_visible = true;
        } else {
            self.panels.record_visible = !self.panels.record_visible;
        }
        if !self.panels.record_visible && self.focus == PanelFocus::Records {
            self.focus = PanelFocus::Chat;
        }
    }

    /// `/swarm`: show or hide the agents panel.
    fn toggle_swarm_panel(&mut self) {
        self.panels.swarm_visible = !self.panels.swarm_visible;
        if !self.panels.swarm_visible && self.focus == PanelFocus::Swarm {
            self.focus = PanelFocus::Chat;
        }
        if self.panels.swarm_visible {
            self.send_command_event(UiEvent::RefreshAgents);
        }
    }

    /// `/files`: show or hide the sandbox panel.
    fn toggle_sandbox_panel(&mut self) {
        self.panels.sandbox_visible = !self.panels.sandbox_visible;
        if self.panels.sandbox_visible {
            self.sandbox.refresh();
        } else {
            // The Record panel gets the column back, and the
            // panel comes back to its own slot next time.
            self.panels.side_expanded = false;
            if self.focus == PanelFocus::Sandbox {
                self.focus = PanelFocus::Chat;
            }
        }
    }

    /// `/bench [new]`: the Bench tab, or its form for a new bench.
    fn bench_command(&mut self, arg: &str) {
        self.show_agents_tab(AgentsTab::Bench);
        if arg.trim() == "new" {
            self.show_bench(BenchMode::Form);
            self.focus = PanelFocus::Chat;
        }
    }
}
