use std::path::PathBuf;

use ferrocull_core::{
    Hook, IngestConfig, NamedProfile, Profile, delete_profile, load_profiles, profiles_dir,
    save_profile,
};
use iced::Task;

use super::Ferrocull;
use crate::messages::{Message, profile};

pub(super) fn update(state: &mut Ferrocull, msg: profile::Message) -> Task<Message> {
    match msg {
        profile::Message::ProfileSelected(name) => state.handle_load_profile(name),
        profile::Message::SaveRequested => return state.handle_save_profile(),
        profile::Message::DeleteRequested(name) => return state.handle_delete_profile(&name),
        profile::Message::NameChanged(name) => state.profile_name_input = name,
        profile::Message::HookAddRequested => state.handle_add_hook(),
        profile::Message::HookRemoved(idx) => {
            state.hooks.remove(idx);
        }
        profile::Message::HookToggled(idx) => {
            state.hooks[idx].enabled = !state.hooks[idx].enabled;
        }
        profile::Message::HookCommandEdited(idx, cmd) => {
            state.hooks[idx].command = cmd;
        }
    }
    Task::none()
}

impl Ferrocull {
    fn handle_load_profile(&mut self, name: String) {
        if let Some(named) = self.profiles.iter().find(|p| p.name == name) {
            self.photos_dest = named.profile.ingest.photos_dest.display().to_string();
            self.videos_dest = named.profile.ingest.videos_dest.display().to_string();
            self.photo_pattern = named.profile.ingest.photo_pattern.clone();
            self.video_pattern = named.profile.ingest.video_pattern.clone();
            self.backup_destinations = named.profile.ingest.backup_destinations.clone();
            self.current_profile = Some(name);
        }
    }

    fn handle_save_profile(&self) -> Task<Message> {
        let name = self.profile_name_input.trim();
        if name.is_empty() {
            return Task::none();
        }
        let name = name.to_owned();
        let profile = Profile {
            ingest: IngestConfig {
                photos_dest: PathBuf::from(&self.photos_dest),
                videos_dest: PathBuf::from(&self.videos_dest),
                photo_pattern: self.photo_pattern.clone(),
                video_pattern: self.video_pattern.clone(),
                backup_destinations: self.backup_destinations.clone(),
            },
        };
        Task::perform(
            tokio::task::spawn_blocking(move || {
                let dir = profiles_dir().map_err(|e| format!("Failed to save profile: {e}"))?;
                save_profile(&name, &profile, &dir)
                    .map_err(|e| format!("Failed to save profile: {e}"))?;
                let profiles =
                    load_profiles(&dir).map_err(|e| format!("Failed to reload profiles: {e}"))?;
                Ok((name, profiles))
            }),
            |r| {
                Message::ProfileSaved(
                    r.unwrap_or_else(|e| Err(format!("profile save panicked: {e}"))),
                )
            },
        )
    }

    fn handle_delete_profile(&self, name: &str) -> Task<Message> {
        let current = self.current_profile.clone();
        let name = name.to_owned();
        Task::perform(
            tokio::task::spawn_blocking(move || {
                let dir = profiles_dir().map_err(|e| format!("Failed to delete profile: {e}"))?;
                delete_profile(&name, &dir)
                    .map_err(|e| format!("Failed to delete profile: {e}"))?;
                let profiles =
                    load_profiles(&dir).map_err(|e| format!("Failed to reload profiles: {e}"))?;
                let new_current = current.filter(|c| c != &name);
                Ok((new_current, profiles))
            }),
            |r| {
                Message::ProfileDeleted(
                    r.unwrap_or_else(|e| Err(format!("profile delete panicked: {e}"))),
                )
            },
        )
    }

    pub(super) fn handle_profile_saved(
        &mut self,
        result: Result<(String, Vec<NamedProfile>), String>,
    ) {
        match result {
            Ok((name, profiles)) => {
                self.profiles = profiles;
                self.current_profile = Some(name);
                self.profile_name_input.clear();
                self.status_message = None;
            }
            Err(e) => self.status_message = Some(e),
        }
    }

    pub(super) fn handle_profile_deleted(
        &mut self,
        result: Result<(Option<String>, Vec<NamedProfile>), String>,
    ) {
        match result {
            Ok((current, profiles)) => {
                self.profiles = profiles;
                self.current_profile = current;
            }
            Err(e) => self.status_message = Some(e),
        }
    }

    fn handle_add_hook(&mut self) {
        let idx = self.hooks.len() + 1;
        self.hooks.push(Hook {
            name: format!("Hook {idx}"),
            command: String::new(),
            enabled: true,
        });
    }
}
