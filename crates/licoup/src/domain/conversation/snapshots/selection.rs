//! Keyword normalization, deterministic profiles, and candidate selection.

use super::*;

pub(super) fn archive_keywords(params: &Value) -> Result<Vec<String>> {
    let mut seen_normalized = BTreeSet::<String>::new();
    let mut keywords = Vec::<String>::new();
    for key in ["keywords", "keyword", "terms", "query", "topic"] {
        let mut raw_keywords = Vec::<String>::new();
        match params.get(key) {
            Some(Value::Array(items)) => {
                for item in items {
                    if let Some(text) = item.as_str() {
                        raw_keywords.extend(split_keyword_list(text));
                    }
                }
            }
            Some(Value::String(text)) => raw_keywords.extend(split_keyword_list(text)),
            _ => {}
        }
        for keyword in raw_keywords {
            let normalized = normalize_match_text(&keyword);
            if !normalized.trim_matches('-').is_empty() && seen_normalized.insert(normalized) {
                keywords.push(keyword);
            }
        }
        if !keywords.is_empty() {
            break;
        }
    }
    if keywords.is_empty() {
        return Err(anyhow!("archive collect requires --keywords"));
    }
    Ok(keywords)
}

pub(super) fn split_keyword_list(value: &str) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect()
}

pub(super) fn derived_archive_profile(
    keywords: &[String],
    archive_root: &Path,
    agents: &[String],
) -> Result<ArchiveProfile> {
    let archive_identity = archive_identity_for_keywords(keywords)?;
    let display_name = archive_identity.display_name;
    let collection_path_segments = archive_identity.collection_path_segments;
    let profile_id = archive_identity.profile_id;
    let canonical_names = archive_identity.canonical_names;
    let alias_names = archive_identity.alias_names;
    let raw = json!({
        "profileId": profile_id,
        "displayName": display_name,
        "collectionPathSegments": collection_path_segments,
        "archiveRoot": display_path(archive_root),
        "canonicalNames": canonical_names,
        "aliasNames": alias_names,
        "projectPaths": [],
        "expectedAgents": agents,
        "expectedSources": [],
        "exclusionRules": []
    });
    parse_archive_profile(&raw)
}

pub(super) fn derived_keyword_archive_profiles(
    keywords: &[String],
    archive_root: &Path,
    agents: &[String],
) -> Result<Vec<ArchiveProfile>> {
    keywords
        .iter()
        .map(|keyword| derived_archive_profile(std::slice::from_ref(keyword), archive_root, agents))
        .collect()
}

pub(super) fn archive_identity_for_keywords(keywords: &[String]) -> Result<DerivedArchiveIdentity> {
    let display_name = keywords.join(", ");
    let collection_path_segments = collection_path_segments_for_keywords(keywords)?;
    let profile_id = collection_path_segments.join("-").trim().to_string();
    if profile_id.is_empty() {
        return Err(anyhow!("archive keywords are empty after normalization"));
    }
    let profile_id = topic_key(&profile_id)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("archive keywords are empty after normalization"))?;
    Ok(DerivedArchiveIdentity {
        profile_id,
        display_name,
        collection_path_segments,
        canonical_names: keywords.to_vec(),
        alias_names: keyword_completion_aliases(keywords),
    })
}

pub(super) fn collection_path_segments_for_keywords(keywords: &[String]) -> Result<Vec<String>> {
    let mut segments = Vec::<String>::new();
    let mut seen = BTreeSet::<String>::new();
    for keyword in keywords {
        let Some(segment) = topic_key(keyword) else {
            continue;
        };
        if !segment.is_empty() && seen.insert(segment.clone()) {
            segments.push(segment);
        }
    }
    if segments.is_empty() {
        Err(anyhow!("archive keywords are empty after normalization"))
    } else {
        Ok(segments)
    }
}

pub(super) fn keyword_completion_aliases(keywords: &[String]) -> Vec<String> {
    let canonical_keys = keywords
        .iter()
        .map(|keyword| normalize_match_text(keyword))
        .collect::<BTreeSet<_>>();
    let mut aliases = BTreeMap::<String, String>::new();
    for keyword in keywords {
        let normalized = normalize_match_text(keyword);
        let compact = compact_identity_key(keyword);
        if !compact.is_empty() && compact != normalized && !canonical_keys.contains(&compact) {
            aliases.entry(compact.clone()).or_insert(compact);
        }
        let camel_spaced = split_camel_word(keyword);
        let camel_normalized = normalize_match_text(&camel_spaced);
        if !camel_normalized.is_empty()
            && camel_normalized != normalized
            && !canonical_keys.contains(&camel_normalized)
        {
            aliases.entry(camel_normalized).or_insert(camel_spaced);
        }
    }
    aliases.into_values().collect()
}

pub(super) fn split_camel_word(value: &str) -> String {
    let mut out = String::new();
    let mut previous_lower_or_digit = false;
    for ch in value.chars() {
        if ch.is_uppercase() && previous_lower_or_digit && !out.ends_with(' ') {
            out.push(' ');
        }
        previous_lower_or_digit = ch.is_lowercase() || ch.is_ascii_digit();
        out.push(ch);
    }
    out
}

pub(super) fn select_profile_archive_candidates(
    profile: &ArchiveProfile,
    discovery: &DiscoveryResult,
) -> (Vec<SelectedCandidate>, BTreeMap<String, ProfileMatch>) {
    let mut selected = Vec::<SelectedCandidate>::new();
    let mut matches = BTreeMap::<String, ProfileMatch>::new();
    let select_all =
        profile.raw.get("selectionMode").and_then(Value::as_str) == Some(ALL_SELECTION);
    let select_exact_keyword =
        profile.raw.get("selectionMode").and_then(Value::as_str) == Some(EXACT_KEYWORD_SELECTION);
    for candidate in &discovery.candidates {
        let Some(id) = candidate_id(candidate) else {
            continue;
        };
        if !candidate_has_real_conversation(candidate) {
            continue;
        }
        let profile_match = if select_all {
            ProfileMatch {
                matched_terms: Vec::new(),
                confidence: "high".to_string(),
                reason: "all local native conversations selected".to_string(),
            }
        } else if select_exact_keyword {
            let Some(profile_match) = exact_keyword_profile_match(candidate, profile) else {
                continue;
            };
            profile_match
        } else {
            let Some(profile_match) = profile_match(candidate, profile) else {
                continue;
            };
            profile_match
        };
        selected.push(SelectedCandidate {
            session: candidate.clone(),
            selection_mode: "deterministic".to_string(),
            reason: profile_match.reason.clone(),
            labels: vec![format!("confidence:{}", profile_match.confidence)],
            group: profile.profile_id.clone(),
            summary: String::new(),
        });
        matches.insert(id, profile_match);
    }
    (selected, matches)
}

fn exact_keyword_profile_match(
    candidate: &Value,
    profile: &ArchiveProfile,
) -> Option<ProfileMatch> {
    let normalized = normalize_match_text(&candidate_exact_keyword_text(candidate));
    let mut matched_terms = profile
        .canonical_names
        .iter()
        .filter(|term| {
            let normalized_term = normalize_match_text(term);
            !normalized_term.is_empty()
                && normalized_contains_identity_term(&normalized, &normalized_term)
        })
        .cloned()
        .collect::<Vec<_>>();
    matched_terms.sort();
    matched_terms.dedup();
    if matched_terms.is_empty() {
        return None;
    }
    Some(ProfileMatch {
        reason: format!(
            "exact conversation keyword matched: {}",
            matched_terms.join(", ")
        ),
        matched_terms,
        confidence: "high".to_string(),
    })
}

pub(super) fn profile_match(candidate: &Value, profile: &ArchiveProfile) -> Option<ProfileMatch> {
    let candidate_text = candidate_search_text(candidate);
    let normalized = normalize_match_text(&candidate_text);
    let candidate_path_text = candidate_path_text(candidate);
    let normalized_path = normalize_match_text(&candidate_path_text);
    let mut matched_terms = Vec::<String>::new();
    let mut matched_identity_keys = BTreeSet::<String>::new();
    let mut path_match = false;
    for term in &profile.project_paths {
        if term.trim().is_empty() {
            continue;
        }
        let normalized_term = normalize_match_text(term);
        if candidate_text.contains(term)
            || candidate_path_text.contains(term)
            || normalized_contains_identity_term(&normalized, &normalized_term)
            || normalized_contains_identity_term(&normalized_path, &normalized_term)
        {
            matched_terms.push(term.clone());
            let identity_key = compact_identity_key(term);
            if !identity_key.is_empty() {
                matched_identity_keys.insert(identity_key);
            }
            path_match = true;
        }
    }
    for term in profile
        .canonical_names
        .iter()
        .chain(profile.alias_names.iter())
    {
        if term.trim().is_empty() {
            continue;
        }
        let normalized_term = normalize_match_text(term);
        if !normalized_term.is_empty()
            && normalized_contains_identity_term(&normalized, &normalized_term)
        {
            matched_terms.push(term.clone());
            let identity_key = compact_identity_key(term);
            if !identity_key.is_empty() {
                matched_identity_keys.insert(identity_key);
            }
        }
    }
    matched_terms.sort();
    matched_terms.dedup();
    if matched_terms.is_empty() {
        return None;
    }
    let confidence = if path_match || matched_identity_keys.len() >= 2 {
        "high"
    } else if profile
        .alias_names
        .iter()
        .any(|term| matched_terms.iter().any(|matched| matched == term))
    {
        "medium"
    } else {
        "low"
    };
    Some(ProfileMatch {
        reason: format!("profile identity matched: {}", matched_terms.join(", ")),
        matched_terms,
        confidence: confidence.to_string(),
    })
}

pub(super) fn topic_key(topic: &str) -> Option<String> {
    let normalized = normalize_match_text(topic).trim_matches('-').to_string();
    if normalized.is_empty() {
        None
    } else if normalized.chars().count() <= 96 {
        Some(normalized)
    } else {
        let digest = hash_text(&normalized);
        Some(format!(
            "{}-{}",
            normalized.chars().take(72).collect::<String>(),
            &digest[..16]
        ))
    }
}

pub(super) fn normalize_match_text(value: &str) -> String {
    let mut out = String::new();
    let mut separator = false;
    for ch in value.chars().filter_map(normalize_width) {
        if ch.is_whitespace() || ch == '-' || ch == '_' {
            separator = true;
            continue;
        }
        if separator && !out.is_empty() {
            out.push('-');
        }
        separator = false;
        for lower in ch.to_lowercase() {
            if lower.is_ascii_alphanumeric() || !lower.is_control() {
                out.push(lower);
            }
        }
    }
    out
}

pub(super) fn compact_identity_key(value: &str) -> String {
    normalize_match_text(value)
        .chars()
        .filter(|ch| *ch != '-')
        .collect()
}

pub(super) fn normalized_contains_identity_term(normalized: &str, term: &str) -> bool {
    if term.is_empty() {
        return false;
    }
    normalized.match_indices(term).any(|(index, _)| {
        let before = normalized[..index].chars().next_back();
        let after = normalized[index + term.len()..].chars().next();
        identity_boundary(before) && identity_boundary(after)
    })
}

pub(super) fn identity_boundary(ch: Option<char>) -> bool {
    ch.map(|ch| !ch.is_ascii_alphanumeric()).unwrap_or(true)
}

pub(super) fn normalize_width(ch: char) -> Option<char> {
    if ch == '\u{3000}' {
        return Some(' ');
    }
    let value = ch as u32;
    if (0xFF01..=0xFF5E).contains(&value) {
        return char::from_u32(value - 0xFEE0);
    }
    Some(ch)
}
