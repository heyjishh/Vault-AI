//! Prompt library (Piece 63, 2026-11-01): a Juris Vault curated
//! collection plus per-user prompts, mirroring the library pattern
//! JISHAN pointed at (Lucio's Prompt Library): categories, search,
//! Preview/Use, star, and share.
//!
//! The curated collection ships in code (fixed ids, no owner) so it
//! survives data wipes and needs no migration. User prompts persist
//! to `prompts.json` in the data directory: the user's own prompts,
//! per-user star sets, and a `shared` flag — a shared prompt appears
//! in every other user's library under "Shared With Me".

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub id: String,
    pub title: String,
    pub body: String,
    pub category: String,
    /// None = curated (Juris Vault collection).
    pub owner_email: Option<String>,
    /// User prompts: visible to other users when true.
    #[serde(default)]
    pub shared: bool,
    /// Curated prompts flagged for the Suggested list.
    #[serde(default)]
    pub suggested: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// What one user sees: the prompt plus their relationship to it.
#[derive(Debug, Clone, Serialize)]
pub struct PromptView {
    #[serde(flatten)]
    pub prompt: Prompt,
    pub mine: bool,
    pub starred: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PromptData {
    prompts: Vec<Prompt>,
    /// user email -> starred prompt ids (curated ids included).
    #[serde(default)]
    stars: HashMap<String, Vec<String>>,
}

pub struct PromptStore {
    data: RwLock<PromptData>,
    path: PathBuf,
}

/// The curated collection. Fixed ids keep stars stable across boots.
pub fn curated() -> Vec<Prompt> {
    let t = DateTime::<Utc>::from_timestamp(1_761_000_000, 0).unwrap_or_default();
    let mk = |id: &str, title: &str, category: &str, suggested: bool, body: &str| Prompt {
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
        mk("curated-redline-issues", "Redline Issues", "Contracts", true,
            "You are acting as lead counsel for [client name]. Review the attached document containing tracked changes (deletions, additions) and comments. For each material change: identify the clause, explain its legal and commercial effect, flag anything that shifts risk to our client, and propose fallback language we can accept. Group the changes by severity and end with the three changes we must not accept as-is."),
        mk("curated-deviation-playbook", "Deviation analysis from playbook", "Contracts", true,
            "You are provided with a playbook and an agreement. Identify and map out every deviation in the agreement from the playbook, clause by clause: quote the playbook position, quote the agreement text, rate each deviation low, medium or high, and state the recommended action (accept, negotiate, or escalate)."),
        mk("curated-insolvency-rights", "Analyze rights on insolvency or bankruptcy", "Banking & Finance", true,
            "Analyze the legal agreement by identifying if it includes provisions that trigger specific rights or consequences upon a party's insolvency or bankruptcy — ipso facto clauses, termination rights, acceleration, set-off and security enforcement. For each right, note how the IBC moratorium under Section 14 and the insolvency process would affect it in practice."),
        mk("curated-dispute-escalation", "Compare dispute escalation procedures", "Dispute Resolution", true,
            "Provide a comprehensive summary comparing dispute escalation procedures from negotiation through to arbitration or litigation. Ensure the summary covers notice requirements, cooling-off periods, mediation, the arbitration seat, venue and rules, and court jurisdiction — citing the exact clause language for each step."),
        mk("curated-summarize-judgment", "Summarize a judgment", "Research", false,
            "Summarize the judgment: facts in brief, the issues framed, each side's arguments, the court's reasoning issue by issue, the ratio decidendi, any obiter worth noting, and the final order. End with a short note on its precedential value and the courts it binds."),
        mk("curated-extract-obligations", "Extract obligations & deadlines", "Contracts", false,
            "Extract every obligation, deadline, notice period and payment term from this agreement into a table: the obligation, the responsible party, the trigger, the due date or period, and the consequence of breach. Flag any obligation that carries no stated consequence."),
        mk("curated-draft-notice", "Draft a legal notice", "Drafting", false,
            "Draft a legal notice on behalf of [client name] to [opposite party] regarding [matter]. Facts: [state the facts]. Structure it with a brief chronology, the legal basis (statute and/or contract clause), the specific demand, a [15]-day compliance window, and the consequences on default. Keep the tone firm and professional, and use [BRACKETED PLACEHOLDERS] for anything not supplied."),
        mk("curated-chronology", "Build a chronology of events", "Dispute Resolution", false,
            "From the documents provided, build a dated chronology of events relevant to [the dispute]. For each event give the date, what happened, the document it comes from, and why it matters legally. Flag any gaps or inconsistencies in the dates."),
        mk("curated-clause-market", "Compare a clause with market standard", "Corporate", false,
            "Compare the [clause type] clause in this agreement against the market-standard position for [transaction type]: what a balanced clause says, where this one deviates, whom the deviation favours, and a redraft that lands at market while protecting our client."),
        mk("curated-dd-red-flags", "Due diligence red flags", "Corporate", false,
            "Review these documents as part of legal due diligence on [target company]. List the red flags by severity: change-of-control triggers, pending litigation or arbitration, regulatory non-compliance, gaps in IP ownership, related-party arrangements, and uncapped liabilities. For each flag, cite the document and the clause or page it comes from."),
        mk("curated-research-memo", "Research memo on a question of law", "Research", false,
            "Prepare a research memo on [question of law] under Indian law: the governing statutory provisions, the leading Supreme Court and High Court authorities with citations, any conflict between High Courts, the current settled position, and a practical conclusion the client can act on."),
        mk("curated-limitation-check", "Limitation period check", "Research", false,
            "For a claim arising from [cause of action] on [date], work out the limitation period under the Limitation Act 1963: the applicable Article of the Schedule, the start date, the exclusions under Sections 12 to 24 that may apply, any acknowledgement under Section 18 that could extend it, and the last date for filing."),
        mk("curated-termination-review", "Employment termination review", "Inhouse Teams", false,
            "Review this termination situation: [describe the facts and paste the relevant contract terms]. Check compliance with the contract's notice and for-cause provisions, the statutory framework that applies (Shops and Establishments / Industrial Disputes, as relevant), full-and-final settlement obligations, and the litigation risk of proceeding as proposed."),
        mk("curated-arbitration-clause", "Stress-test an arbitration clause", "Dispute Resolution", false,
            "Stress-test this arbitration clause for enforceability and traps: is the agreement to arbitrate clear, are the seat and venue consistent, is the appointment mechanism valid after the 2015/2019 amendments and the Perkins Eastman line of cases, are excepted matters carved out, and does the clause survive the main contract? Propose a corrected clause."),
    ]
}

impl PromptStore {
    pub fn load(path: PathBuf) -> Self {
        let data: PromptData = std::fs::read(&path)
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

    /// Everything one user may see: curated + their own + prompts
    /// other users shared. Suggested first, then newest.
    pub fn list_for(&self, user_email: &str) -> Vec<PromptView> {
        let data = self.data.read().unwrap();
        let starred: Vec<&str> = data
            .stars
            .get(user_email)
            .map(|v| v.iter().map(String::as_str).collect())
            .unwrap_or_default();
        let view = |p: &Prompt| PromptView {
            mine: p.owner_email.as_deref() == Some(user_email),
            starred: starred.contains(&p.id.as_str()),
            prompt: p.clone(),
        };
        let mut out: Vec<PromptView> = curated().iter().map(view).collect();
        out.extend(
            data.prompts
                .iter()
                .filter(|p| {
                    p.owner_email.as_deref() == Some(user_email) || p.shared
                })
                .map(view),
        );
        out.sort_by(|a, b| {
            b.prompt
                .suggested
                .cmp(&a.prompt.suggested)
                .then(b.prompt.updated_at.cmp(&a.prompt.updated_at))
                .then(a.prompt.title.cmp(&b.prompt.title))
        });
        out
    }

    pub fn create(
        &self,
        user_email: &str,
        title: &str,
        body: &str,
        category: &str,
    ) -> Prompt {
        let now = Utc::now();
        let fresh = Prompt {
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
            data.prompts.push(fresh.clone());
        }
        self.save();
        fresh
    }

    /// Update one of the user's own prompts; None when it is not
    /// theirs (or not a user prompt at all).
    pub fn update(
        &self,
        user_email: &str,
        id: &str,
        title: Option<&str>,
        body: Option<&str>,
        category: Option<&str>,
    ) -> Option<Prompt> {
        let updated = {
            let mut data = self.data.write().unwrap();
            let p = data.prompts.iter_mut().find(|p| {
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
            let before = data.prompts.len();
            data.prompts.retain(|p| {
                !(p.id == id && p.owner_email.as_deref() == Some(user_email))
            });
            data.prompts.len() != before
        };
        if removed {
            self.save();
        }
        removed
    }

    /// Toggle a star for any prompt the user can see (curated ids
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

    /// Toggle sharing on one of the user's own prompts.
    pub fn toggle_share(&self, user_email: &str, id: &str) -> Option<bool> {
        let state = {
            let mut data = self.data.write().unwrap();
            let p = data.prompts.iter_mut().find(|p| {
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

    fn store() -> PromptStore {
        let path = std::env::temp_dir()
            .join(format!("prompts-test-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        PromptStore::load(path)
    }

    #[test]
    fn curated_visible_to_everyone_and_starrable() {
        let s = store();
        let list = s.list_for("a@test.io");
        assert!(list.len() >= 10, "curated seeds present");
        assert!(list.iter().all(|v| !v.mine));
        let id = list[0].prompt.id.clone();
        assert!(s.toggle_star("a@test.io", &id));
        let after = s.list_for("a@test.io");
        assert!(after.iter().find(|v| v.prompt.id == id).unwrap().starred);
    }

    #[test]
    fn own_prompts_private_until_shared() {
        let s = store();
        let p = s.create("a@test.io", "Mine", "Body", "Contracts");
        assert!(s
            .list_for("b@test.io")
            .iter()
            .all(|v| v.prompt.id != p.id));
        assert_eq!(s.toggle_share("a@test.io", &p.id), Some(true));
        assert!(s
            .list_for("b@test.io")
            .iter()
            .any(|v| v.prompt.id == p.id && !v.mine));
        // Another user cannot edit or delete it.
        assert!(s.update("b@test.io", &p.id, Some("X"), None, None).is_none());
        assert!(!s.delete("b@test.io", &p.id));
        assert!(s.delete("a@test.io", &p.id));
    }
}
