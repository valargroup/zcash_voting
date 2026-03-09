// One-shot endpoint to delete all bot messages from Slack and clear state.
//
// Usage:
//   curl -X POST https://<your-app>.vercel.app/api/reset
//
// This deletes every parent message the bot posted (which also removes
// all thread replies), then wipes the notifier-state in Edge Config so
// the next poll re-discovers all PRs and posts fresh messages with the
// current SLACK_MENTION_IDS.
//
// Requires the same env vars as the poll endpoint.
// The Slack app also needs the chat:write scope (already granted) —
// bots can delete their own messages without extra scopes.

import { loadConfig } from './_lib/config';
import { loadState, saveState } from './_lib/store';

export const config = { runtime: 'edge' };

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

async function deleteSlackMessage(
  token: string,
  channel: string,
  ts: string,
): Promise<boolean> {
  const resp = await fetch('https://slack.com/api/chat.delete', {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json; charset=utf-8',
    },
    body: JSON.stringify({ channel, ts }),
  });
  const result = (await resp.json()) as { ok: boolean; error?: string };
  return result.ok;
}

export default async function handler(req: Request) {
  if (req.method !== 'POST') {
    return json({ error: 'Send a POST request to reset' }, 405);
  }

  const cfg = loadConfig();
  if ('error' in cfg) {
    return json({ error: cfg.error }, 500);
  }

  const state = await loadState();
  const log: string[] = [];

  for (const [key, tracked] of Object.entries(state.trackedPrs)) {
    const ok = await deleteSlackMessage(
      cfg.slackBotToken,
      cfg.slackChannelId,
      tracked.slackThreadTs,
    );
    log.push(`${key}: ${ok ? 'deleted' : 'failed (may already be deleted)'}`);
  }

  const cleared = Object.keys(state.trackedPrs).length;
  state.trackedPrs = {};
  state.lastDiscoveryPoll = new Date(0).toISOString();
  state.lastReconcilePoll = new Date(0).toISOString();

  try {
    await saveState(state, cfg.vercelApiToken, cfg.edgeConfigId);
  } catch (err) {
    log.push(`State save failed: ${err}`);
    return json({ error: 'Failed to save state', log }, 500);
  }

  return json({
    status: 'reset',
    messagesDeleted: cleared,
    log,
  });
}
