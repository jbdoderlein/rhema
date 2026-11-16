//! Session state: resume, clear, export, the checkpoint the exit
//! save rides, and the gauges and palette snapshots read from it.

use super::*;

impl App {
    /// Backs the outgoing conversation up, then drops it: a fresh
    /// transcript and a fresh session. The file binding drops with it,
    /// what is typed after /clear is a new conversation, not a
    /// continuation of the backed-up file, so the next turn allocates a
    /// file of its own instead of overwriting that history (see
    /// [`Self::ensure_checkpoint_target`]).
    pub fn clear_session(&mut self) {
        // The backup lands before the history clears: a clear is a
        // conversation boundary the user asked for, not a discard.
        let backup = self.back_up_session();
        if let Some(agent) = self.agent.as_mut() {
            agent.session_mut().history.clear();
        }
        // The App's own copy clears with the conversation too: with the
        // agent out, a keyless switch, a removed login, this copy is
        // what a switch-back rebuilds on and what the exit save exports.
        // Left full, the cleared conversation came back, and the exit
        // save (its file binding dropped below) wrote it to a fresh
        // session file: one conversation, two picker entries.
        self.session.history.clear();
        self.transcript = Transcript::new();
        self.usage = None;
        self.session_cost = 0.0;
        self.error = None;
        self.retryable = false;
        self.session_file = None;
        // The benchmark describes one conversation: cleared with it,
        // in the fallback snapshot and in the agent's mirror alike.
        self.transcript_stats = None;
        if let Some(agent) = self.agent.as_mut() {
            agent.reset_transcript_stats();
        }
        match backup {
            Some(Err(err)) => self.transcript.push_notice(&format!(
                "conversation cleared; the old one was not saved: {err}"
            )),
            Some(Ok(_)) => self
                .transcript
                .push_notice("conversation saved; starting fresh"),
            None => self.transcript.push_notice("conversation cleared"),
        }
    }

    /// Writes the conversation out before [`Self::clear_session`] drops
    /// it: into this conversation's own file, the one it was resumed
    /// from or the runner allocated, so the run's `--save` path stays
    /// free for the conversation that follows. A conversation with no
    /// file (a run whose startup allocation failed) gets a fresh one in
    /// the sessions cache. `None` when the history is empty, so there is
    /// nothing to back up; `Some(Err)` when the write failed, the clear
    /// proceeds either way and says so.
    fn back_up_session(&self) -> Option<std::io::Result<PathBuf>> {
        let (session, _) = self.export_session()?;
        let saved = match self.session_file.clone() {
            Some(path) => crate::sessions::write_session(&path, &session).map(|()| path),
            None => self
                .sessions_base()
                .and_then(|base| self.save_new_session(&base, &session)),
        };
        Some(saved)
    }

    /// Allocates a file under `base` and writes `session` there. The
    /// same shape [`crate::sessions::save_session`] takes, but the path
    /// comes back: the caller just proved the history is not empty.
    fn save_new_session(&self, base: &Path, session: &Session) -> std::io::Result<PathBuf> {
        let workspace = crate::args::canonical(&self.options.workspace);
        let path = crate::sessions::next_session_path(base, &workspace)?;
        crate::sessions::write_session(&path, session)?;
        Ok(path)
    }

    /// The sessions cache: the setup's own base (the test isolation
    /// seam), else the environment's.
    fn sessions_base(&self) -> std::io::Result<PathBuf> {
        match &self.sessions_base {
            Some(base) => Ok(base.clone()),
            None => crate::sessions::base_dir(),
        }
    }

    /// A fresh file for the next conversation, named from the sessions
    /// cache. `None` when the cache is unreachable, the turn then runs
    /// uncheckpointed rather than failing.
    fn next_session_file(&self) -> Option<PathBuf> {
        let base = self.sessions_base().ok()?;
        let workspace = crate::args::canonical(&self.options.workspace);
        crate::sessions::next_session_path(&base, &workspace).ok()
    }

    /// The file a turn checkpoints into and the exit save updates:
    /// `--save`'s explicit path wins; otherwise this conversation's own
    /// file. `None` when neither exists, the exit save then falls back
    /// to allocating at exit, and a turn simply cannot checkpoint.
    pub fn checkpoint_target(&self) -> Option<PathBuf> {
        self.options
            .save
            .clone()
            .or_else(|| self.session_file.clone())
    }

    /// The checkpoint target a turn takes: [`Self::checkpoint_target`],
    /// allocated first when the conversation has none. `/clear` drops
    /// the binding so the next conversation does not overwrite the
    /// backed-up file; its first turn then gets a fresh name (unless
    /// `--save` names the run's file) and streams to disk from then on.
    pub fn ensure_checkpoint_target(&mut self) -> Option<PathBuf> {
        if self.checkpoint_target().is_none() {
            self.session_file = self.next_session_file();
        }
        self.checkpoint_target()
    }

    /// The session to save for /resume, and the file it belongs to: the
    /// live session when its history holds anything, writing back to the
    /// origin when this session was resumed. A session of notices alone
    /// (a switch, a decline) has nothing resumable and saves nothing.
    pub fn export_session(&self) -> Option<(Session, Option<PathBuf>)> {
        let mut session = match &self.agent {
            Some(agent) => agent.session().clone(),
            None => self.session.clone(),
        };
        // The accounting rides with the save: a resumed session's
        // footer continues where this one ended, and its Usage page
        // keeps this run's numbers.
        session.cost_usd = self.session_cost;
        crate::sessions::UsageStats {
            transcript: self
                .agent
                .as_ref()
                .and_then(CodingAgent::transcript_stats)
                .or(self.transcript_stats),
            // The compaction rides with the counters: the resumed
            // session adopts the sections instead of re-summarizing.
            transcript_sections: self
                .agent
                .as_ref()
                .map(CodingAgent::transcript_sections)
                .filter(|sections| !sections.is_empty()),
            last_turn: self.usage,
        }
        .write_into(&mut session);
        if session.history.is_empty() {
            None
        } else {
            Some((session, self.session_file.clone()))
        }
    }

    /// Opens the resume picker over the sessions saved for this
    /// workspace. An empty history of saves is a notice, not a screen.
    pub fn open_resume(&mut self) -> Vec<Effect> {
        match crate::sessions::base_dir() {
            Ok(base) => self.open_resume_from(&base),
            Err(err) => {
                self.push_notice(&format!("cannot locate the session cache: {err}"));
                Vec::new()
            }
        }
    }

    /// [`App::open_resume`] against an explicit base, so tests drive real
    /// directories instead of the environment.
    pub fn open_resume_from(&mut self, base: &Path) -> Vec<Effect> {
        if self.turn.running {
            self.transcript
                .push_notice("a turn is running; /resume waits for it to finish");
            return Vec::new();
        }
        let workspace = crate::args::canonical(&self.options.workspace);
        match crate::sessions::list_sessions(base, &workspace) {
            Ok(entries) if entries.is_empty() => {
                self.transcript
                    .push_notice("no saved sessions for this directory");
            }
            Ok(entries) => {
                self.screen = Screen::Resume(crate::screens::ResumeScreen::new(
                    entries,
                    crate::sessions::now_millis(),
                ));
            }
            Err(err) => self.error = Some(format!("cannot list saved sessions: {err}")),
        }
        Vec::new()
    }

    /// The `/resume` pick: refuses politely under a running turn, and
    /// hands the load to the runner, the session file is a JSON read
    /// plus a parse on the blocking pool, and the result lands as
    /// [`UiEvent::SessionResumed`] for `apply_resume`.
    pub fn request_resume(&mut self, path: &Path) -> Vec<Effect> {
        if self.turn.running {
            self.transcript
                .push_notice("a turn is running; /resume waits for it to finish");
            return Vec::new();
        }
        vec![Effect::ResumeSession(path.to_path_buf())]
    }

    /// Loads a saved session and applies it on the spot: the
    /// synchronous path the tests drive and the headless runner keeps.
    /// The interactive app goes through [`Self::request_resume`]
    /// instead, the UI thread never reads the file.
    pub fn resume_session(&mut self, path: &Path) {
        let loaded = Self::load_resume_parts(path).map_err(|err| err.to_string());
        self.apply_resume(path, loaded);
    }

    /// Applies a loaded resume: rebuilds the agent on it, re-derives
    /// the transcript from its history, and lands in the chat. The
    /// current conversation is replaced, it keeps its own saved file
    /// from its own exit.
    pub(crate) fn apply_resume(&mut self, path: &Path, loaded: Result<Box<ResumedLoad>, String>) {
        let loaded = match loaded {
            Ok(loaded) => *loaded,
            Err(err) => {
                self.error = Some(format!("cannot load session: {err}"));
                return;
            }
        };
        let ResumedLoad { session, stats } = loaded;
        let (mut agent, subagents, missing) = match self.build_agent(session.clone()) {
            Ok(built) => built,
            Err(err) => {
                self.error = Some(err.to_string());
                return;
            }
        };
        let messages = session.history.len();
        self.session = session;
        // The saved usage accounting came with the load: the last
        // turn's tokens go straight to the gauge, and the transcript's
        // counters fold into the fresh mirror, so /settings's Usage
        // page keeps reading as one run.
        self.usage = stats.as_ref().and_then(|stats| stats.last_turn);
        if let Some(saved) = stats.as_ref().and_then(|stats| stats.transcript.as_ref()) {
            agent.carry_transcript_stats(saved);
        }
        if let Some(sections) = stats
            .as_ref()
            .and_then(|stats| stats.transcript_sections.clone())
        {
            agent.adopt_transcript_sections(sections);
        }
        self.transcript_stats = None;
        self.agent = Some(agent);
        self.subagents = Some(subagents);
        self.missing_tools = missing;
        self.session_file = Some(path.to_path_buf());
        self.transcript = Transcript::from_history(&self.session.history);
        // The saved session carries what it spent; the footer continues
        // the story instead of quoting a fresh zero.
        self.session_cost = self.session.cost_usd;
        self.retryable = false;
        self.chat.scroll = 0;
        self.screen = Screen::Chat;
        self.transcript
            .push_notice(&format!("resumed {messages} messages"));
    }

    /// Reads and derives a resume's parts: the session file plus the
    /// usage accounting computed from it, the work the UI thread must
    /// not do. Runs on the blocking pool via
    /// [`crate::turn::spawn_session_resume`].
    pub(crate) fn load_resume_parts(path: &Path) -> std::io::Result<Box<ResumedLoad>> {
        let session = CodingAgent::load_session_from(&path.to_path_buf())?;
        let stats = crate::sessions::UsageStats::from_session(&session);
        Ok(Box::new(ResumedLoad { session, stats }))
    }

    /// The settings that apply to this session's profile: field-wise, the
    /// named profile (or the one chosen at startup) over the top-level
    /// section. Computed on demand, it changes mid-session.
    pub fn effective(&self) -> rhema_config::Profile {
        self.config.effective(self.session_profile.as_deref())
    }

    /// The keychain this session resolves stored keys through, as the
    /// resolution tier under the environment; `None` reads literals only.
    pub fn secret_store(&self) -> Option<&dyn rhema_config::secrets::SecretStore> {
        self.secrets.as_deref()
    }

    /// The profiles the input palette completes `/profile` with: the
    /// config registry's rows, the session's own marked.
    pub fn palette_profiles(&self) -> Vec<crate::screens::PaletteProfile> {
        self.config
            .profiles
            .iter()
            .map(|(name, profile)| crate::screens::PaletteProfile {
                name: name.clone(),
                provider: profile.provider.clone(),
                model: profile.model.clone(),
                active: self.session_profile.as_deref() == Some(name.as_str()),
            })
            .collect()
    }

    /// The sandbox instances the input palette completes `/sandbox`
    /// with: the session profile's configured instances, the one the
    /// session currently runs `bash` in marked.
    pub fn palette_sandboxes(&self) -> Vec<crate::screens::PaletteSandbox> {
        let effective = self.config.effective(self.session_profile.as_deref());
        let active_name = effective
            .sandbox
            .as_ref()
            .and_then(|sandbox| sandbox.active.clone());
        let running = self.active_sandbox();
        effective
            .sandbox_instances
            .iter()
            .map(|(name, instance)| crate::screens::PaletteSandbox {
                name: name.clone(),
                kind: instance.kind.clone(),
                target: instance
                    .target
                    .clone()
                    .map(|target| target.trim().to_owned())
                    .filter(|target| !target.is_empty()),
                active: active_name.as_deref() == Some(name.as_str())
                    && SandboxChoice::from_stored(Some(&instance.kind)) == Some(running),
            })
            .collect()
    }

    /// The active profile's name and display color: the `--profile` flag
    /// over the stored active profile, colored by its config (a `#RRGGBB`
    /// string) or by a name-based theme tone.
    pub fn active_profile(&self) -> Option<(String, ratatui::style::Color)> {
        let name = self.session_profile.clone()?;
        let custom = self
            .config
            .profiles
            .get(&name)
            .and_then(|profile| profile.color.as_deref())
            .and_then(crate::palette::parse_hex_color);
        let color =
            custom.unwrap_or_else(|| crate::palette::default_profile_color(&name, &self.theme));
        Some((name, color))
    }

    /// The pricing of the model the session currently runs on, when its
    /// adapter prices it.
    pub(crate) fn current_model_pricing(&self) -> Option<rhema_provider::Pricing> {
        let effective = self.effective();
        let model = effective.model.as_deref()?;
        let active = self.backend.as_ref().map(Backend::config_name);
        self.models
            .iter()
            .find(|entry| entry.model.id == model && Some(entry.provider) == active)
            .and_then(|entry| entry.model.pricing)
    }

    /// The background shells this session started, as the last poll saw
    /// them: running ones only, oldest first.
    pub(crate) fn job_entries(&self) -> Vec<crate::jobs::JobEntry> {
        self.jobs
            .as_ref()
            .map(crate::jobs::JobsHandle::entries)
            .unwrap_or_default()
    }

    /// The background subagent jobs' live state, read off the wire's
    /// registry handle: one entry per working job, queued or running.
    /// A finished job left already; its outcome arrived as a finish
    /// event. Empty without an agent or a wire.
    pub(crate) fn subagent_jobs(&self) -> Vec<rhema_subagents::JobStatus> {
        self.subagents
            .as_ref()
            .map(|handle| handle.status())
            .unwrap_or_default()
    }

    /// The pricing of any catalog model by id, the running subagent's
    /// model may differ from the session's, and its cost prices by its
    /// own card. An unknown id (the catalog never fetched it) prices to
    /// nothing.
    pub(crate) fn model_pricing(&self, model: &str) -> Option<rhema_provider::Pricing> {
        self.models
            .iter()
            .find(|entry| entry.model.id == model)
            .and_then(|entry| entry.model.pricing)
    }

    /// The context window of any catalog model by id, the running
    /// subagent's gauge is sized by its own card. `None` when unknown.
    pub(crate) fn model_window(&self, model: &str) -> Option<u64> {
        self.models
            .iter()
            .find(|entry| entry.model.id == model)
            .and_then(|entry| entry.model.input_token_limit)
    }

    /// The context window of the current model, when known.
    pub fn current_context_window(&self) -> Option<u64> {
        let effective = self.effective();
        let model = effective.model.as_deref()?;
        let active = self.backend.as_ref().map(Backend::config_name);
        self.models
            .iter()
            .find(|entry| entry.model.id == model && Some(entry.provider) == active)
            .and_then(|entry| entry.model.input_token_limit)
    }

    /// The context footprint the footer shows: the latest reported
    /// tally, the last turn's final usage, or while a turn streams the
    /// provider's running count, whose prompt already includes its reply
    ///, otherwise an estimate over the loaded history, which already
    /// includes every message. A resumed session has spent context
    /// before its first turn here, and an empty one estimates to zero.
    pub fn context_used(&self) -> u64 {
        match &self.usage {
            Some(usage) => context_tokens(usage),
            None => estimate_history_tokens(&self.live_session().history),
        }
    }

    /// The session the conversation currently lives in: the agent's when
    /// one is built, the bare one otherwise.
    pub(crate) fn live_session(&self) -> &Session {
        match &self.agent {
            Some(agent) => agent.session(),
            None => &self.session,
        }
    }
}
