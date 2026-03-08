// Vercel cron: poll GitHub for tracked PRs and notify Slack.
//
// Runs every 5 minutes (see vercel.json). Three phases per invocation:
//
//   1. Discovery — search for open PRs by tracked authors in tracked orgs.
//      New PRs get a parent Slack message in #ext-zcash-contrib-notif.
//
//   2. Event fetch — for each tracked open PR, fetch issue comments,
//      review comments, and review submissions since the last poll.
//      External events (not from tracked authors) post as Slack thread
//      replies under the PR's parent message.
//
//   3. Reconciliation — once per 24 h, re-check closed/merged PRs for
//      reopen events. If a PR reopens, the parent Slack message is
//      updated and active polling resumes.
//
// Required env vars: see _lib/config.ts

import type { NotifierConfig } from './_lib/config';
import { loadConfig, renderingConfigHash } from './_lib/config';
import type { TrackedPR, NotifierState } from './_lib/store';
import { loadState, saveState, prKey } from './_lib/store';
import {
  searchOpenPRs,
  parseRepoFromUrl,
  fetchIssueComments,
  fetchReviewComments,
  fetchReviews,
  fetchPRState,
} from './_lib/github';
import {
  postParentMessage,
  updateParentMessage,
  postThreadReply,
  formatQuote,
} from './_lib/slack';

export const config = { runtime: 'edge' };

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

// ---------- Phase 1: Discovery ----------

async function discover(
  cfg: NotifierConfig,
  state: NotifierState,
  log: string[],
): Promise<boolean> {
  let changed = false;

  for (const author of cfg.trackedAuthors) {
    let prs;
    try {
      prs = await searchOpenPRs(author, cfg.trackedOrgs, cfg.githubToken);
    } catch (err) {
      log.push(`Discovery error for ${author}: ${err}`);
      continue;
    }

    for (const pr of prs) {
      const { owner, repo } = parseRepoFromUrl(pr.repository_url);
      const key = prKey(owner, repo, pr.number);
      const prUrl = pr.pull_request?.html_url ?? pr.html_url;

      // Already tracked — update title if it changed.
      const existing = state.trackedPrs[key];
      if (existing) {
        if (existing.title !== pr.title) {
          existing.title = pr.title;
          changed = true;
        }
        // Discovered as open but we had it as closed/merged — it was reopened.
        if (existing.state !== 'open') {
          existing.state = 'open';
          changed = true;
          if (!cfg.dryRun) {
            await updateParentMessage(
              cfg.slackBotToken,
              cfg.slackChannelId,
              existing.slackThreadTs,
              { ...existing },
              cfg.slackMentionIds,
              cfg.authorSlackMap,
            );
            await postThreadReply(
              cfg.slackBotToken,
              cfg.slackChannelId,
              existing.slackThreadTs,
              'PR was *reopened*.',
              cfg.slackMentionIds,
            );
          }
          log.push(`Reopened (via discovery): ${key}`);
        }
        continue;
      }

      // New PR — post parent Slack message.
      if (cfg.dryRun) {
        log.push(`[DRY RUN] Would post parent message for ${key}`);
        continue;
      }

      const ts = await postParentMessage(
        cfg.slackBotToken,
        cfg.slackChannelId,
        { owner, repo, number: pr.number, title: pr.title, author: pr.user.login, url: prUrl, state: 'open' },
        cfg.slackMentionIds,
        cfg.authorSlackMap,
      );

      if (ts) {
        state.trackedPrs[key] = {
          owner,
          repo,
          number: pr.number,
          author: pr.user.login,
          title: pr.title,
          url: prUrl,
          state: 'open',
          slackThreadTs: ts,
          lastEventAt: new Date().toISOString(),
          discoveredAt: new Date().toISOString(),
        };
        changed = true;
        log.push(`New PR tracked: ${key}`);
      } else {
        log.push(`Slack post failed for ${key}`);
      }
    }
  }

  return changed;
}

// ---------- Phase 1.5: Auto-refresh on config change ----------

async function autoRefresh(
  cfg: NotifierConfig,
  state: NotifierState,
  log: string[],
): Promise<boolean> {
  const currentHash = renderingConfigHash(cfg);
  if (state.configHash === currentHash) return false;
  if (Object.keys(state.trackedPrs).length === 0) {
    state.configHash = currentHash;
    return true;
  }

  log.push(`Config changed (${state.configHash ?? 'none'} → ${currentHash}), refreshing parent messages`);

  for (const [key, tracked] of Object.entries(state.trackedPrs)) {
    if (cfg.dryRun) {
      log.push(`[DRY RUN] Would refresh ${key}`);
      continue;
    }
    const ok = await updateParentMessage(
      cfg.slackBotToken,
      cfg.slackChannelId,
      tracked.slackThreadTs,
      { ...tracked },
      cfg.slackMentionIds,
      cfg.authorSlackMap,
    );
    log.push(`${key}: ${ok ? 'refreshed' : 'refresh failed'}`);
  }

  state.configHash = currentHash;
  return true;
}

// ---------- Phase 2: Event fetch for open PRs ----------

interface ExternalEvent {
  at: string;
  text: string;
}

function reviewStateLabel(state: string): string {
  switch (state) {
    case 'APPROVED':
      return '✅ approved';
    case 'CHANGES_REQUESTED':
      return '🔄 requested changes';
    case 'COMMENTED':
      return '💬 reviewed';
    case 'DISMISSED':
      return '❌ dismissed';
    default:
      return state.toLowerCase();
  }
}

async function fetchEvents(
  cfg: NotifierConfig,
  state: NotifierState,
  log: string[],
): Promise<boolean> {
  let changed = false;
  const trackedAuthorsLower = new Set(
    cfg.trackedAuthors.map((a) => a.toLowerCase()),
  );

  for (const [key, tracked] of Object.entries(state.trackedPrs)) {
    if (tracked.state !== 'open') continue;

    try {
      const [issueComments, reviewComments, reviews] = await Promise.all([
        fetchIssueComments(tracked.owner, tracked.repo, tracked.number, tracked.lastEventAt, cfg.githubToken),
        fetchReviewComments(tracked.owner, tracked.repo, tracked.number, tracked.lastEventAt, cfg.githubToken),
        fetchReviews(tracked.owner, tracked.repo, tracked.number, cfg.githubToken),
      ]);

      const since = new Date(tracked.lastEventAt);
      const events: ExternalEvent[] = [];

      for (const c of issueComments) {
        if (trackedAuthorsLower.has(c.user.login.toLowerCase())) continue;
        if (new Date(c.created_at) <= since) continue;
        events.push({
          at: c.created_at,
          text: `💬 *${c.user.login}* <${c.html_url}|commented>:\n${formatQuote(c.body, 300)}`,
        });
      }

      for (const c of reviewComments) {
        if (trackedAuthorsLower.has(c.user.login.toLowerCase())) continue;
        if (new Date(c.created_at) <= since) continue;
        events.push({
          at: c.created_at,
          text: `📝 *${c.user.login}* <${c.html_url}|left an inline comment>:\n${formatQuote(c.body, 300)}`,
        });
      }

      for (const r of reviews) {
        if (trackedAuthorsLower.has(r.user.login.toLowerCase())) continue;
        if (new Date(r.submitted_at) <= since) continue;
        const bodyPart = r.body ? `:\n${formatQuote(r.body, 300)}` : '';
        events.push({
          at: r.submitted_at,
          text: `🔍 *${r.user.login}* <${r.html_url}|${reviewStateLabel(r.state)}>${bodyPart}`,
        });
      }

      events.sort((a, b) => a.at.localeCompare(b.at));

      let latestEventAt = tracked.lastEventAt;
      for (const event of events) {
        if (cfg.dryRun) {
          log.push(`[DRY RUN] Thread reply in ${key}: ${event.text.slice(0, 80)}`);
        } else {
          const ok = await postThreadReply(
            cfg.slackBotToken,
            cfg.slackChannelId,
            tracked.slackThreadTs,
            event.text,
            cfg.slackMentionIds,
          );
          if (!ok) {
            log.push(`Thread reply failed for ${key}, stopping batch`);
            break;
          }
        }
        if (event.at > latestEventAt) latestEventAt = event.at;
      }

      if (latestEventAt !== tracked.lastEventAt) {
        tracked.lastEventAt = latestEventAt;
        changed = true;
      }

      // Check for state transitions (open -> closed/merged).
      const current = await fetchPRState(
        tracked.owner,
        tracked.repo,
        tracked.number,
        cfg.githubToken,
      );
      if (current && current.state !== tracked.state) {
        changed = true;
        await handleStateTransition(cfg, tracked, current.state, current.title, log);
      }
    } catch (err) {
      log.push(`Error processing ${key}: ${err}`);
    }
  }

  return changed;
}

// ---------- Phase 3: Reconciliation ----------

async function reconcile(
  cfg: NotifierConfig,
  state: NotifierState,
  log: string[],
): Promise<boolean> {
  const elapsed =
    Date.now() - new Date(state.lastReconcilePoll).getTime();
  if (elapsed < cfg.reconcileIntervalMs) return false;

  for (const [key, tracked] of Object.entries(state.trackedPrs)) {
    if (tracked.state === 'open') continue;

    try {
      const current = await fetchPRState(
        tracked.owner,
        tracked.repo,
        tracked.number,
        cfg.githubToken,
      );
      if (current && current.state !== tracked.state) {
        await handleStateTransition(cfg, tracked, current.state, current.title, log);
        log.push(`Reconcile: ${key} → ${current.state}`);
      }
    } catch (err) {
      log.push(`Reconcile error for ${key}: ${err}`);
    }
  }

  state.lastReconcilePoll = new Date().toISOString();
  return true;
}

// ---------- Shared: state transition ----------

async function handleStateTransition(
  cfg: NotifierConfig,
  tracked: TrackedPR,
  newState: 'open' | 'closed' | 'merged',
  newTitle: string,
  log: string[],
): Promise<void> {
  const oldState = tracked.state;
  tracked.state = newState;
  tracked.title = newTitle;

  if (cfg.dryRun) {
    log.push(
      `[DRY RUN] State change ${tracked.owner}/${tracked.repo}#${tracked.number}: ${oldState} → ${newState}`,
    );
    return;
  }

  await updateParentMessage(
    cfg.slackBotToken,
    cfg.slackChannelId,
    tracked.slackThreadTs,
    { ...tracked },
    cfg.slackMentionIds,
    cfg.authorSlackMap,
  );
  await postThreadReply(
    cfg.slackBotToken,
    cfg.slackChannelId,
    tracked.slackThreadTs,
    `PR status changed: *${oldState}* → *${newState}*`,
    [],
  );
}

// ---------- Handler ----------

export default async function handler(req: Request) {
  if (req.method !== 'GET') {
    return json({ error: 'Method not allowed' }, 405);
  }

  const cfg = loadConfig();
  if ('error' in cfg) {
    return json({ error: cfg.error }, 500);
  }

  const state = await loadState();
  const log: string[] = [];

  const refreshChanged = await autoRefresh(cfg, state, log);
  const discoveryChanged = await discover(cfg, state, log);
  const eventsChanged = await fetchEvents(cfg, state, log);
  const reconcileChanged = await reconcile(cfg, state, log);

  state.lastDiscoveryPoll = new Date().toISOString();

  if (refreshChanged || discoveryChanged || eventsChanged || reconcileChanged) {
    try {
      await saveState(state, cfg.vercelApiToken, cfg.edgeConfigId);
    } catch (err) {
      log.push(`State save failed: ${err}`);
      return json({ error: 'Failed to save state', log }, 500);
    }
  }

  return json({
    status: 'ok',
    trackedOpen: Object.values(state.trackedPrs).filter((p) => p.state === 'open').length,
    trackedTotal: Object.keys(state.trackedPrs).length,
    refreshed: refreshChanged,
    reconciled: reconcileChanged,
    log,
  });
}
