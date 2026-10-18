//! Watch-mode rebuild and safe extension snapshot reload orchestration.

use std::path::{Path, PathBuf};

use rintawa_artifacts::ArtifactDigest;
use rintawa_extension_engine::ExtensionState;
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};

use crate::{DevError, DevProject, DevResult, DevSession, SourceRevision};

/// Result of one reload attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// Rebuilding produced identical RTW bytes.
    Unchanged,
    /// A new snapshot replaced the running snapshot.
    Reloaded {
        /// Previous artifact digest.
        previous: ArtifactDigest,
        /// Current artifact digest.
        current: ArtifactDigest,
    },
}

/// Reloadable development session for one project.
pub struct ReloadingDevSession {
    project_path: PathBuf,
    instance_id: ExtensionInstanceId,
    scope_id: RuntimeScopeId,
    session: Option<DevSession>,
}

impl ReloadingDevSession {
    /// Starts a reloadable project session.
    ///
    /// # Errors
    ///
    /// Returns an error if the initial snapshot cannot be prepared or started.
    pub fn start(
        path: impl AsRef<Path>,
        instance_id: ExtensionInstanceId,
        scope_id: RuntimeScopeId,
    ) -> DevResult<Self> {
        let project = DevProject::open(path)?;
        let project_path = project.root().to_path_buf();
        let session = DevSession::start(&project, instance_id.clone(), scope_id.clone())?;
        Ok(Self {
            project_path,
            instance_id,
            scope_id,
            session: Some(session),
        })
    }

    /// Returns the current source revision.
    ///
    /// # Errors
    ///
    /// Returns an error if the project cannot be reopened or scanned.
    pub fn source_revision(&self) -> DevResult<SourceRevision> {
        DevProject::open(&self.project_path)?.source_revision()
    }

    /// Returns the running session, if available.
    pub fn session(&self) -> Option<&DevSession> {
        self.session.as_ref()
    }

    /// Rebuilds and replaces the running snapshot when its bytes changed.
    ///
    /// Build and packing failures leave the current session running. If the new
    /// snapshot fails to start, the previous immutable snapshot is restored.
    ///
    /// # Errors
    ///
    /// Returns a preparation error, a reload error after successful rollback, or
    /// both reload and rollback errors when neither snapshot can be started.
    pub fn reload(&mut self) -> DevResult<ReloadOutcome> {
        let project = DevProject::open(&self.project_path)?;
        let next_snapshot = project.prepare_snapshot()?;
        let current = self.session.as_ref().ok_or(DevError::SessionUnavailable)?;
        if current.digest() == next_snapshot.digest() {
            return Ok(ReloadOutcome::Unchanged);
        }

        let previous_digest = current.digest().clone();
        let current_digest = next_snapshot.digest().clone();
        let previous_session = self.session.take().ok_or(DevError::SessionUnavailable)?;
        let previous_snapshot = previous_session.shutdown_into_snapshot()?;

        match DevSession::start_snapshot(
            next_snapshot,
            self.instance_id.clone(),
            self.scope_id.clone(),
        ) {
            Ok(session) => {
                self.session = Some(session);
                Ok(ReloadOutcome::Reloaded {
                    previous: previous_digest,
                    current: current_digest,
                })
            }
            Err(reload_error) => match DevSession::start_snapshot(
                previous_snapshot,
                self.instance_id.clone(),
                self.scope_id.clone(),
            ) {
                Ok(previous_session) => {
                    self.session = Some(previous_session);
                    Err(DevError::ReloadFailed {
                        source: Box::new(reload_error),
                    })
                }
                Err(rollback_error) => Err(DevError::ReloadAndRollbackFailed {
                    reload: Box::new(reload_error),
                    rollback: Box::new(rollback_error),
                }),
            },
        }
    }

    /// Returns the current runtime state.
    pub fn state(&self) -> Option<ExtensionState> {
        self.session.as_ref().and_then(DevSession::state)
    }

    /// Stops the running session.
    ///
    /// # Errors
    ///
    /// Returns an engine cleanup error.
    pub fn shutdown(mut self) -> DevResult<()> {
        match self.session.take() {
            Some(session) => session.shutdown(),
            None => Ok(()),
        }
    }
}
