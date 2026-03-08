// Re-render all tracked parent Slack messages with the current config.
//
// Usage:
//   curl -X POST https://<your-app>.vercel.app/api/refresh
//
// Updates every parent message in-place using the current SLACK_MENTION_IDS
// and PR state without deleting or re-posting anything. Thread history and
// tracked state are preserved.

import { loadConfig } from './_lib/config';
import { loadState, saveState } from './_lib/store';
import { updateParentMessage } from './_lib/slack';

export const config = { runtime: 'edge' };

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

export default async function handler(req: Request) {
  if (req.method !== 'POST') {
    return json({ error: 'Send a POST request to refresh' }, 405);
  }

  const cfg = loadConfig();
  if ('error' in cfg) {
    return json({ error: cfg.error }, 500);
  }

  const state = await loadState();
  const log: string[] = [];
  let updated = 0;

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

    if (ok) {
      updated++;
      log.push(`${key}: refreshed`);
    } else {
      log.push(`${key}: update failed`);
    }
  }

  return json({
    status: 'refreshed',
    updated,
    total: Object.keys(state.trackedPrs).length,
    log,
  });
}
