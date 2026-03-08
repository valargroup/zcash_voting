// Slack API helpers for the PR notifier.
//
// Uses raw fetch against chat.postMessage / chat.update rather than
// the @slack/web-api SDK so the code runs in Vercel's Edge runtime
// without extra dependencies.

// ---------- Types ----------

interface SlackApiResult {
  ok: boolean;
  ts?: string;
  error?: string;
}

interface SlackBlock {
  type: string;
  text?: { type: string; text: string; emoji?: boolean };
}

// ---------- Helpers ----------

async function slackApi(
  method: string,
  token: string,
  body: Record<string, unknown>,
): Promise<SlackApiResult> {
  const resp = await fetch(`https://slack.com/api/${method}`, {
    method: 'POST',
    headers: {
      Authorization: `Bearer ${token}`,
      'Content-Type': 'application/json; charset=utf-8',
    },
    body: JSON.stringify(body),
  });
  return (await resp.json()) as SlackApiResult;
}

function stateEmoji(state: string): string {
  switch (state) {
    case 'open':
      return '🟢';
    case 'merged':
      return '🟣';
    case 'closed':
      return '🔴';
    default:
      return '⚪';
  }
}

function mentionSuffix(ids: string[]): string {
  if (ids.length === 0) return '';
  return '\n' + ids.map((id) => `<@${id}>`).join(' ');
}

export function escapeSlack(text: string): string {
  return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

export function formatQuote(body: string, maxLen: number): string {
  const trimmed = body.length > maxLen ? body.slice(0, maxLen) + '…' : body;
  return trimmed
    .split('\n')
    .map((line) => `> ${escapeSlack(line)}`)
    .join('\n');
}

// ---------- Parent message ----------

function formatAuthor(author: string, authorSlackMap: Record<string, string>): string {
  const slackId = authorSlackMap[author.toLowerCase()];
  return slackId ? `<@${slackId}>` : `*${escapeSlack(author)}*`;
}

function parentBlocks(
  pr: { owner: string; repo: string; number: number; title: string; author: string; url: string; state: string },
  mentionIds: string[],
  authorSlackMap: Record<string, string>,
): SlackBlock[] {
  const mentions = mentionIds.length > 0
    ? `\ncc ${mentionIds.map((id) => `<@${id}>`).join(' ')}`
    : '';
  return [
    {
      type: 'section',
      text: {
        type: 'mrkdwn',
        text:
          `${stateEmoji(pr.state)} *<${pr.url}|${pr.owner}/${pr.repo}#${pr.number}>*: ${escapeSlack(pr.title)}\n` +
          `Author: ${formatAuthor(pr.author, authorSlackMap)} · Status: *${pr.state}*${mentions}`,
      },
    },
  ];
}

export async function postParentMessage(
  token: string,
  channel: string,
  pr: { owner: string; repo: string; number: number; title: string; author: string; url: string; state: string },
  mentionIds: string[],
  authorSlackMap: Record<string, string> = {},
): Promise<string | null> {
  const result = await slackApi('chat.postMessage', token, {
    channel,
    text: `New PR: ${pr.owner}/${pr.repo}#${pr.number} – ${pr.title}`,
    blocks: parentBlocks(pr, mentionIds, authorSlackMap),
    unfurl_links: false,
    unfurl_media: false,
  });
  return result.ok ? (result.ts ?? null) : null;
}

export async function updateParentMessage(
  token: string,
  channel: string,
  ts: string,
  pr: { owner: string; repo: string; number: number; title: string; author: string; url: string; state: string },
  mentionIds: string[],
  authorSlackMap: Record<string, string> = {},
): Promise<boolean> {
  const result = await slackApi('chat.update', token, {
    channel,
    ts,
    text: `PR ${pr.state}: ${pr.owner}/${pr.repo}#${pr.number} – ${pr.title}`,
    blocks: parentBlocks(pr, mentionIds, authorSlackMap),
  });
  return result.ok;
}

// ---------- Thread replies ----------

export async function postThreadReply(
  token: string,
  channel: string,
  threadTs: string,
  text: string,
  mentionIds: string[],
): Promise<boolean> {
  const result = await slackApi('chat.postMessage', token, {
    channel,
    thread_ts: threadTs,
    text: text + mentionSuffix(mentionIds),
    unfurl_links: false,
    unfurl_media: false,
  });
  return result.ok;
}
