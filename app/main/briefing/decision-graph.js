function createDecisionGraphReader({
  fs,
  path,
  BRIEFINGS_DIR,
  validDateStamp,
}) {
  function readJsonFileIfPresent(file) {
    if (!fs.existsSync(file)) return null;
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

  function readJsonlFileIfPresent(file) {
    if (!fs.existsSync(file)) return { exists: false, items: [] };
    const raw = fs.readFileSync(file, 'utf8');
    const items = raw
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter(Boolean)
      .map((line) => JSON.parse(line));
    return { exists: true, items };
  }

  function decisionGraphDomains(profileSnapshot, domainRows, decisions) {
    const ordered = [];
    const seen = new Set();
    const push = (id, enabled = true) => {
      const value = String(id || '').trim();
      if (!value || seen.has(value) || enabled === false) return;
      seen.add(value);
      ordered.push(value);
    };

    const profileDomains = profileSnapshot
      && profileSnapshot.profile
      && Array.isArray(profileSnapshot.profile.domains)
      ? profileSnapshot.profile.domains
      : [];
    profileDomains
      .slice()
      .sort((a, b) => Number(a.priority || 0) - Number(b.priority || 0))
      .forEach((domain) => push(domain.name || domain.id, domain.enabled));
    domainRows.forEach((domain) => push(domain.id || domain.name));
    decisions.forEach((decision) => {
      push(decision.domain);
      (decision.cross_domain || []).forEach((domain) => push(domain));
    });
    return ordered.length ? ordered : ['Career', 'Health', 'Family'];
  }

  function fallbackDecisionGraphEdges(decisions) {
    const edges = [];
    const seen = new Set();
    const ids = new Set(decisions.map((decision) => decision.id).filter(Boolean));
    const add = (fromId, toId, metadata = {}) => {
      if (!fromId || !toId || !ids.has(fromId) || !ids.has(toId)) return;
      const key = `${fromId}->${toId}`;
      if (seen.has(key)) return;
      seen.add(key);
      edges.push({
        from_id: fromId,
        to_id: toId,
        relation: metadata.relation || metadata.mechanism || 'caused',
        mechanism: metadata.mechanism || metadata.rationale || '',
        strength: metadata.strength || 'contributing',
        rationale: metadata.rationale || null,
        derived_from_decisions: true,
      });
    };

    decisions.forEach((decision) => {
      (decision.causes || []).forEach((cause) => {
        if (typeof cause === 'string') {
          add(cause, decision.id);
        } else if (cause && typeof cause === 'object') {
          add(cause.from_id || cause.id, cause.to_id || decision.id, cause);
        }
      });
      (decision.effects || []).forEach((effect) => {
        if (typeof effect === 'string') {
          add(decision.id, effect);
        } else if (effect && typeof effect === 'object') {
          add(effect.from_id || decision.id, effect.to_id || effect.id, effect);
        }
      });
    });
    return edges;
  }

  function readDecisionGraphForDate(date) {
    if (!validDateStamp(date)) {
      return { ok: false, error: 'invalid date' };
    }
    const dir = path.join(BRIEFINGS_DIR, date);
    const decisionsPath = path.join(dir, 'decisions.jsonl');
    const edgesPath = path.join(dir, 'tree', 'L4-edges.jsonl');
    const domainsPath = path.join(dir, 'tree', 'L4-domains.jsonl');
    const profilePath = path.join(dir, 'synthesis-profile.snapshot.json');
    try {
      const decisions = readJsonlFileIfPresent(decisionsPath);
      if (!decisions.exists) {
        return { ok: false, date, error: 'No decision artifacts found for this day.' };
      }
      const edgeRows = readJsonlFileIfPresent(edgesPath);
      const domainRows = readJsonlFileIfPresent(domainsPath);
      const profileSnapshot = readJsonFileIfPresent(profilePath);
      const fallbackEdges = edgeRows.exists ? [] : fallbackDecisionGraphEdges(decisions.items);
      const edges = edgeRows.exists ? edgeRows.items : fallbackEdges;
      const domains = decisionGraphDomains(profileSnapshot, domainRows.items, decisions.items);
      return {
        ok: true,
        date,
        paths: {
          decisions: decisionsPath,
          edges: edgeRows.exists ? edgesPath : null,
          domains: domainRows.exists ? domainsPath : null,
          profile: profileSnapshot ? profilePath : null,
        },
        decisions: decisions.items,
        edges,
        domains,
        derived_edges: fallbackEdges.length,
        summary: {
          decision_count: decisions.items.length,
          edge_count: edges.length,
          domain_count: domains.length,
        },
      };
    } catch (e) {
      return { ok: false, date, error: e.message };
    }
  }

  return {
    readJsonFileIfPresent,
    readJsonlFileIfPresent,
    decisionGraphDomains,
    fallbackDecisionGraphEdges,
    readDecisionGraphForDate,
  };
}

module.exports = { createDecisionGraphReader };
