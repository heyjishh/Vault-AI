//! Template library (Piece 68, 2026-11-01): the drafting-template
//! twin of the prompt library — a Juris Vault curated collection
//! plus per-user templates, with categories, search, Preview/Use,
//! star, and share. Templates live in Draft only (JISHAN's call):
//! the sidebar carries the library, and the Draft composer's +
//! menu is the one place a template can be picked into a document.
//!
//! The curated collection ships in code (fixed ids, no owner) so it
//! survives data wipes and needs no migration. User templates
//! persist to `templates.json` in the data directory: the user's
//! own templates, per-user star sets, and a `shared` flag — a
//! shared template appears in every other user's library under
//! "Shared With Me".

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Template {
    pub id: String,
    pub title: String,
    pub body: String,
    pub category: String,
    /// None = curated (Juris Vault collection).
    pub owner_email: Option<String>,
    /// User templates: visible to other users when true.
    #[serde(default)]
    pub shared: bool,
    /// Curated templates flagged for the Suggested list.
    #[serde(default)]
    pub suggested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What one user sees: the template plus their relationship to it.
#[derive(Debug, Clone, Serialize)]
pub struct TemplateView {
    #[serde(flatten)]
    pub template: Template,
    pub mine: bool,
    pub starred: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct TemplateData {
    templates: Vec<Template>,
    /// user email -> starred template ids (curated ids included).
    #[serde(default)]
    stars: HashMap<String, Vec<String>>,
}

pub struct TemplateStore {
    data: RwLock<TemplateData>,
    path: PathBuf,
}

/// The curated collection. Fixed ids keep stars stable across boots.
pub fn curated() -> Vec<Template> {
    let t = DateTime::<Utc>::from_timestamp(1_761_000_000, 0).unwrap_or_default();
    let mk = |id: &str, title: &str, category: &str, suggested: bool, body: &str| Template {
        id: id.to_string(),
        title: title.to_string(),
        body: body.to_string(),
        category: category.to_string(),
        owner_email: None,
        shared: false,
        suggested,
        created_at: t,
        updated_at: t,
    };
    vec![
        mk("tpl-legal-notice-dues", "Legal notice — unpaid dues", "Notices", true,
            "Draft a legal notice on behalf of [client name] to [opposite party] demanding payment of \u{20B9}[amount] due under [invoice / agreement dated]. Structure: brief chronology of the transaction and the default; the amount with interest computed at [rate]% from [due date]; a clear demand for payment within 15 days of receipt; and the consequences on default — recovery proceedings before [forum], further interest and costs. Keep the tone firm and professional; use [BRACKETED PLACEHOLDERS] for facts not supplied."),
        mk("tpl-cease-desist", "Cease & desist — IP infringement", "Notices", false,
            "Draft a cease-and-desist notice on behalf of [client name], owner of [trade mark / copyright work, with registration details], against [infringer] for [describe the infringing use]. Set out the client's rights, the acts complained of with dates, the demand to stop use and withdraw the infringing material within [7] days, an undertaking to be signed, and the proceedings that will follow on non-compliance."),
        mk("tpl-rent-notice", "Notice to tenant — default & vacation", "Notices", false,
            "Draft a notice to a tenant at [premises address] for default in payment of rent of \u{20B9}[amount] per month for [number] months. Recite the tenancy terms, compute the arrears, demand payment within 15 days and vacant possession on continued default, and reserve the landlord's rights under the [rent control / Transfer of Property Act] framework and the tenancy agreement."),
        mk("tpl-mutual-nda", "Mutual non-disclosure agreement", "Agreements", true,
            "Draft a mutual NDA between [party A] and [party B] for discussions about [purpose]. Include: a two-way definition of Confidential Information with the usual carve-outs; permitted use and disclosure to representatives on a need-to-know basis; a [3]-year confidentiality period; return or destruction of materials on request; exceptions for legal compulsion with notice; governing law [India] and jurisdiction [courts of place]. Proportionate, market-standard drafting — no one-sided traps."),
        mk("tpl-services-agreement", "Services agreement — MSA skeleton", "Agreements", false,
            "Draft a master services agreement between [client] and [service provider] for [services]. Cover: scope and statements of work; fees, invoicing and GST; service levels and credits; IP — work product vests in the client, provider background IP licensed as needed; confidentiality; data protection per the DPDP Act 2023; liability cap at [cap]; term, termination for convenience on [30] days and for cause; dispute resolution by [arbitration seated at place / courts]."),
        mk("tpl-agreement-to-sell", "Agreement to sell — property skeleton", "Agreements", false,
            "Draft an agreement to sell for the property at [address, with survey / khasra details] from [seller] to [purchaser] for a total consideration of \u{20B9}[amount]. Include: title declarations and the seller's ownership warranty; earnest money of \u{20B9}[amount] and the payment schedule; conditions precedent (title search, approvals); the completion date and registration at [Sub-Registrar office]; default and forfeiture terms kept within enforceable limits; and possession delivery on registration."),
        mk("tpl-plaint-recovery", "Plaint — recovery suit skeleton", "Pleadings", true,
            "Draft a plaint for a recovery suit of \u{20B9}[amount] before the [court] at [place], [plaintiff] versus [defendant]. Follow Order VII CPC structure: parties and their descriptions; the transaction and cause of action with dates; jurisdiction and valuation with court fee; limitation compliance; the default and the legal notice dated [date]; interest pendente lite and future interest under Section 34 CPC; and a prayer clause covering the decree, interest, costs and any other relief."),
        mk("tpl-written-statement", "Written statement skeleton", "Pleadings", false,
            "Draft a written statement for the defendant in [suit details] following Order VIII CPC: preliminary objections (maintainability, limitation, jurisdiction, suppression) first; then para-wise replies that admit, deny or lack knowledge of each plaint paragraph, with specific denials as Order VIII Rule 5 requires; the defendant's own version of facts; any set-off or counterclaim; and the verification. Leave [BRACKETED PLACEHOLDERS] wherever the client's instructions are needed."),
        mk("tpl-affidavit-evidence", "Affidavit of evidence (examination-in-chief)", "Affidavits", true,
            "Draft an affidavit of evidence for [witness name] in [case details], as examination-in-chief under Order XVIII Rule 4 CPC. First person throughout: identity and how the witness knows the facts; the facts in chronological order, each paragraph numbered and within personal knowledge; documents exhibited and marked; and the standard verification at [place] on [date]. Flag with [BRACKETS] anything that risks being inadmissible hearsay so counsel can settle it."),
        mk("tpl-board-resolution", "Board resolution — authorisation", "Corporate", false,
            "Draft a certified board resolution of [company name] (CIN [number]) passed at its board meeting on [date] at [place]: RESOLVED THAT consent of the Board be and is hereby accorded to [file / defend / execute — the act]; RESOLVED FURTHER THAT [director / officer name] be and is hereby authorised severally to sign, execute and do all acts and deeds for giving effect to the above; with the standard certified-true-copy footer for the Company Secretary / Director."),
        mk("tpl-employment-offer", "Employment offer letter", "Employment", false,
            "Draft an employment offer letter from [company] to [candidate] for the role of [title] at [location], joining on [date]. Cover: CTC of \u{20B9}[amount] with the break-up in an annexure; probation of [6] months and confirmation; notice period of [30/90] days on either side; leave and working hours per policy; confidentiality and IP assignment; and acceptance signature block. Keep statutory references (PF, gratuity, ESI as applicable) accurate and brief."),
        mk("tpl-termination-letter", "Termination letter — as per contract", "Employment", false,
            "Draft a termination letter for [employee name], [title], strictly as per clause [number] of the employment agreement: the contractual basis for termination, the effective last working day of [date] after the [notice period / pay in lieu], handover obligations, full-and-final settlement timeline, and return of company property and confidential material. Professional and non-argumentative tone; no reasons beyond what the contract requires."),
    ]
}

impl TemplateStore {
    pub fn load(path: PathBuf) -> Self {
        let data: TemplateData = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { data: RwLock::new(data), path }
    }

    fn save(&self) {
        if let Ok(data) = self.data.read() {
            if let Ok(bytes) = serde_json::to_vec_pretty(&*data) {
                let tmp = self.path.with_extension("json.tmp");
                if std::fs::write(&tmp, bytes).is_ok() {
                    let _ = std::fs::rename(tmp, &self.path);
                }
            }
        }
    }

    /// Everything one user may see: curated + their own + templates
    /// other users shared. Suggested first, then newest.
    pub fn list_for(&self, user_email: &str) -> Vec<TemplateView> {
        let data = self.data.read().unwrap();
        let starred: Vec<&str> = data
            .stars
            .get(user_email)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let view = |p: &Template| TemplateView {
            mine: p.owner_email.as_deref() == Some(user_email),
            starred: starred.contains(&p.id.as_str()),
            template: p.clone(),
        };
        let mut out: Vec<TemplateView> = curated().iter().map(view).collect();
        out.extend(
            data.templates
                .iter()
                .filter(|p| {
                    p.owner_email.as_deref() == Some(user_email) || p.shared
                })
                .map(view),
        );
        out.sort_by(|a, b| {
            b.template
                .suggested
                .cmp(&a.template.suggested)
                .then(b.template.updated_at.cmp(&a.template.updated_at))
                .then(a.template.title.cmp(&b.template.title))
        });
        out
    }

    pub fn create(
        &self,
        user_email: &str,
        title: &str,
        body: &str,
        category: &str,
    ) -> Template {
        let now = Utc::now();
        let fresh = Template {
            id: legal_rag_common::new_id(),
            title: title.to_string(),
            body: body.to_string(),
            category: if category.trim().is_empty() {
                "General".to_string()
            } else {
                category.trim().to_string()
            },
            owner_email: Some(user_email.to_string()),
            shared: false,
            suggested: false,
            created_at: now,
            updated_at: now,
        };
        {
            let mut data = self.data.write().unwrap();
            data.templates.push(fresh.clone());
        }
        self.save();
        fresh
    }

    /// Update one of the user's own templates; None when it is not
    /// theirs (or not a user template at all).
    pub fn update(
        &self,
        user_email: &str,
        id: &str,
        title: Option<&str>,
        body: Option<&str>,
        category: Option<&str>,
    ) -> Option<Template> {
        let updated = {
            let mut data = self.data.write().unwrap();
            let p = data.templates.iter_mut().find(|p| {
                p.id == id && p.owner_email.as_deref() == Some(user_email)
            })?;
            if let Some(t) = title {
                p.title = t.to_string();
            }
            if let Some(b) = body {
                p.body = b.to_string();
            }
            if let Some(c) = category {
                p.category = c.to_string();
            }
            p.updated_at = Utc::now();
            p.clone()
        };
        self.save();
        Some(updated)
    }

    pub fn delete(&self, user_email: &str, id: &str) -> bool {
        let removed = {
            let mut data = self.data.write().unwrap();
            let before = data.templates.len();
            data.templates.retain(|p| {
                !(p.id == id && p.owner_email.as_deref() == Some(user_email))
            });
            data.templates.len() != before
        };
        if removed {
            self.save();
        }
        removed
    }

    /// Toggle a star for any template the user can see (curated ids
    /// work too). Returns the new starred state.
    pub fn toggle_star(&self, user_email: &str, id: &str) -> bool {
        let now_starred = {
            let mut data = self.data.write().unwrap();
            let set = data.stars.entry(user_email.to_string()).or_default();
            if set.iter().any(|s| s == id) {
                set.retain(|s| s != id);
                false
            } else {
                set.push(id.to_string());
                true
            }
        };
        self.save();
        now_starred
    }

    /// Toggle sharing on one of the user's own templates.
    pub fn toggle_share(&self, user_email: &str, id: &str) -> Option<bool> {
        let state = {
            let mut data = self.data.write().unwrap();
            let p = data.templates.iter_mut().find(|p| {
                p.id == id && p.owner_email.as_deref() == Some(user_email)
            })?;
            p.shared = !p.shared;
            p.shared
        };
        self.save();
        Some(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> TemplateStore {
        let path = std::env::temp_dir()
            .join(format!("templates-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        TemplateStore::load(path)
    }

    #[test]
    fn curated_visible_to_everyone_and_starrable() {
        let s = store();
        let list = s.list_for("a@test.io");
        assert!(list.len() >= 10, "curated seeds present");
        assert!(list.iter().all(|v| !v.mine));
        let id = list[0].template.id.clone();
        assert!(s.toggle_star("a@test.io", &id));
        let after = s.list_for("a@test.io");
        assert!(after.iter().find(|v| v.template.id == id).unwrap().starred);
    }

    #[test]
    fn own_templates_private_until_shared() {
        let s = store();
        let p = s.create("a@test.io", "Mine", "Body", "Contracts");
        assert!(s
            .list_for("b@test.io")
            .iter()
            .all(|v| v.template.id != p.id));
        assert_eq!(s.toggle_share("a@test.io", &p.id), Some(true));
        assert!(s
            .list_for("b@test.io")
            .iter()
            .any(|v| v.template.id == p.id && !v.mine));
        // Another user cannot edit or delete it.
        assert!(s.update("b@test.io", &p.id, Some("X"), None, None).is_none());
        assert!(!s.delete("b@test.io", &p.id));
        assert!(s.delete("a@test.io", &p.id));
    }
}
