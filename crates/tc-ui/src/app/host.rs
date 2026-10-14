//! What commands (GUI menus, CLI, MCP, links) can do in the window.

use super::*;

impl Host for Core {
    fn store(&mut self) -> &mut SessionStore {
        &mut self.store
    }

    fn persist(&mut self) -> Result<(), CommandError> {
        self.store
            .save(&self.paths.sessions_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tab_list(&self) -> Result<Vec<TabInfo>, CommandError> {
        Ok(self
            .tabs
            .iter()
            .map(|t| TabInfo {
                id: t.id,
                title: t.title.clone(),
                active: Some(t.id) == self.active,
            })
            .collect())
    }

    fn tab_open(&mut self, session: Option<Session>) -> Result<u64, CommandError> {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        let tab = self.new_tab(id, session);
        self.tabs.push(tab);
        self.active = Some(id);
        Ok(id)
    }

    fn tab_send(&mut self, tab: Option<u64>, text: &str) -> Result<(), CommandError> {
        let i = self.tab_index(tab)?;
        self.tabs[i].send(text.as_bytes());
        Ok(())
    }

    fn tab_close(&mut self, tab: Option<u64>) -> Result<(), CommandError> {
        let i = self.tab_index(tab)?;
        let id = self.tabs.remove(i).id;
        if self.active == Some(id) {
            // Focus the neighbour on the left, else the one on the right.
            self.active = self
                .tabs
                .get(i.saturating_sub(1))
                .or_else(|| self.tabs.get(i))
                .map(|t| t.id);
        }
        Ok(())
    }

    fn tab_duplicate(&mut self, tab: Option<u64>) -> Result<u64, CommandError> {
        let i = self.tab_index(tab)?;
        let session = self.tabs[i].session.clone();
        self.tab_open(session)
    }

    fn vault(&self) -> Result<&tc_vault::Vault, CommandError> {
        Ok(&self.vault)
    }

    fn known_hosts_file(&self) -> Result<std::path::PathBuf, CommandError> {
        Ok(self.paths.known_hosts_file())
    }

    fn macros(&mut self) -> Result<&mut tc_core::MacroStore, CommandError> {
        Ok(&mut self.macros)
    }

    fn persist_macros(&mut self) -> Result<(), CommandError> {
        self.macros
            .save(&self.paths.macros_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tunnels(&mut self) -> Result<&mut TunnelStore, CommandError> {
        Ok(&mut self.tunnels)
    }

    fn persist_tunnels(&mut self) -> Result<(), CommandError> {
        self.tunnels
            .save(&self.paths.tunnels_file())
            .map_err(|e| CommandError::Failed(e.to_string()))
    }

    fn tunnel_status(&self, id: TunnelId) -> Option<String> {
        self.running.get(&id).map(|t| t.status.clone())
    }

    fn server_list(&self) -> Result<Vec<tc_commands::ServerInfo>, CommandError> {
        Ok(self.servers.list())
    }

    fn server_start(&mut self, server: &str) -> Result<String, CommandError> {
        let shared = self.paths.data_dir.join("shared");
        self.servers.start(server, &self.settings.servers, &shared)
    }

    fn server_stop(&mut self, server: &str) -> Result<(), CommandError> {
        self.servers.stop(server)
    }

    fn tunnel_start(&mut self, tunnel: Tunnel) -> Result<(), CommandError> {
        let running = RunningTunnel::start(
            &tunnel,
            self.paths.known_hosts_file(),
            self.vault.clone(),
            &self.settings.passwords,
            crate::tab::agent_config(&self.settings.ssh),
            self.repaint.clone(),
        );
        self.running.insert(tunnel.id, running);
        Ok(())
    }

    fn tunnel_stop(&mut self, id: TunnelId) -> Result<(), CommandError> {
        self.running
            .remove(&id)
            .map(drop)
            .ok_or_else(|| CommandError::NotFound(format!("tunnel {id} is not running")))
    }

    fn tab_rename(&mut self, tab: Option<u64>, title: &str) -> Result<(), CommandError> {
        let i = self.tab_index(tab)?;
        let title = title.trim();
        if title.is_empty() {
            return Err(CommandError::InvalidArgs("title must not be empty".into()));
        }
        self.tabs[i].title = title.to_string();
        Ok(())
    }
}
