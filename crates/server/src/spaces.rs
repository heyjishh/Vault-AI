//! Spaces — multi-tenant containers above Briefs.
//!
//! A Space has an owner and members with roles (`owner` / `editor` /
//! `viewer`). A Brief filed in a Space is visible only to that Space's
//! members: listing, detail, scoped Ask retrieval, Draft grounding and
//! Vault filing all enforce membership. Briefs with no Space keep the
//! legacy behaviour (visible to every authenticated user), so existing
//! deployments are unaffected until they adopt Spaces.
//!
//! Roles: viewer = read + scoped queries; editor = viewer + upload /
//! edit briefs; owner = editor + manage members + delete the space.
//! Platform admins bypass membership (they administer the deployment).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const ROLE_OWNER: &str = "owner";
pub const ROLE_EDITOR: &str = "editor";
pub const ROLE_VIEWER: &str = "viewer";

pub fn valid_role(role: &str) -> bool {
    matches!(role, ROLE_OWNER | ROLE_EDITOR | ROLE_VIEWER)
}

/// Can this role change content (upload, edit briefs)?
pub fn role_can_edit(role: &str) -> bool {
    matches!(role, ROLE_OWNER | ROLE_EDITOR)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpaceMember {
    pub user_id: String,
    pub email: String,
    pub role: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    #[serde(default)]
    pub members: Vec<SpaceMember>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Space {
    /// The member's role in this space, if a member at all.
    pub fn role_of(&self, user_id: &str) -> Option<&str> {
        self.members
            .iter()
            .find(|m| m.user_id == user_id)
            .map(|m| m.role.as_str())
    }
}

pub struct SpaceStore {
    spaces: RwLock<Vec<Space>>,
    path: PathBuf,
}

impl SpaceStore {
    pub fn load(path: PathBuf) -> Self {
        let spaces: Vec<Space> = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self {
            spaces: RwLock::new(spaces),
            path,
        }
    }

    fn save(&self) {
        if let Ok(spaces) = self.spaces.read() {
            if let Ok(bytes) = serde_json::to_vec(&*spaces) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    pub fn create(&self, name: &str, owner_id: &str, owner_email: &str) -> Space {
        let now = Utc::now();
        let space = Space {
            id: legal_rag_common::new_id(),
            name: name.to_string(),
            owner_id: owner_id.to_string(),
            members: vec![SpaceMember {
                user_id: owner_id.to_string(),
                email: owner_email.to_string(),
                role: ROLE_OWNER.to_string(),
            }],
            created_at: now,
            updated_at: now,
        };
        self.spaces.write().unwrap().push(space.clone());
        self.save();
        space
    }

    pub fn get(&self, id: &str) -> Option<Space> {
        self.spaces
            .read()
            .unwrap()
            .iter()
            .find(|s| s.id == id)
            .cloned()
    }

    /// Every space the user belongs to.
    pub fn for_user(&self, user_id: &str) -> Vec<Space> {
        let mut out: Vec<Space> = self
            .spaces
            .read()
            .unwrap()
            .iter()
            .filter(|s| s.role_of(user_id).is_some())
            .cloned()
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    pub fn list_all(&self) -> Vec<Space> {
        self.spaces.read().unwrap().clone()
    }

    pub fn rename(&self, id: &str, name: &str) -> Option<Space> {
        let mut spaces = self.spaces.write().unwrap();
        let space = spaces.iter_mut().find(|s| s.id == id)?;
        space.name = name.to_string();
        space.updated_at = Utc::now();
        let out = space.clone();
        drop(spaces);
        self.save();
        Some(out)
    }

    pub fn delete(&self, id: &str) -> bool {
        let mut spaces = self.spaces.write().unwrap();
        let before = spaces.len();
        spaces.retain(|s| s.id != id);
        let removed = spaces.len() != before;
        drop(spaces);
        if removed {
            self.save();
        }
        removed
    }

    /// Add a member or change their role. The space owner can never be
    /// demoted or displaced through this path.
    pub fn upsert_member(&self, id: &str, user_id: &str, email: &str, role: &str) -> Option<Space> {
        let mut spaces = self.spaces.write().unwrap();
        let space = spaces.iter_mut().find(|s| s.id == id)?;
        if space.owner_id == user_id && role != ROLE_OWNER {
            return Some(space.clone()); // owner keeps the owner role
        }
        if let Some(m) = space.members.iter_mut().find(|m| m.user_id == user_id) {
            m.role = role.to_string();
            m.email = email.to_string();
        } else {
            space.members.push(SpaceMember {
                user_id: user_id.to_string(),
                email: email.to_string(),
                role: role.to_string(),
            });
        }
        space.updated_at = Utc::now();
        let out = space.clone();
        drop(spaces);
        self.save();
        Some(out)
    }

    /// Remove a member. The owner cannot be removed.
    pub fn remove_member(&self, id: &str, user_id: &str) -> Option<Space> {
        let mut spaces = self.spaces.write().unwrap();
        let space = spaces.iter_mut().find(|s| s.id == id)?;
        if space.owner_id == user_id {
            return None;
        }
        space.members.retain(|m| m.user_id != user_id);
        space.updated_at = Utc::now();
        let out = space.clone();
        drop(spaces);
        self.save();
        Some(out)
    }

    /// Map of space id → the user's role, for every space they belong to.
    pub fn roles_for(&self, user_id: &str) -> HashMap<String, String> {
        self.spaces
            .read()
            .unwrap()
            .iter()
            .filter_map(|s| s.role_of(user_id).map(|r| (s.id.clone(), r.to_string())))
            .collect()
    }
}

/// Access decision for one Brief, given the caller's space roles.
/// `is_admin` (platform admin) bypasses membership entirely.
pub fn can_read(space_id: Option<&str>, roles: &HashMap<String, String>, is_admin: bool) -> bool {
    match space_id {
        None => true, // legacy brief: no space boundary
        Some(sid) => is_admin || roles.contains_key(sid),
    }
}

/// Edit decision for one Brief: editor+ in its space (or legacy rules:
/// any authenticated user, as before Spaces existed).
pub fn can_edit(space_id: Option<&str>, roles: &HashMap<String, String>, is_admin: bool) -> bool {
    match space_id {
        None => true,
        Some(sid) => {
            is_admin || roles.get(sid).map(|r| role_can_edit(r)).unwrap_or(false)
        }
    }
}
