// Notifier configuration loaded from environment variables.
//
// Required env vars (server-side only):
//   SLACK_BOT_TOKEN            — Slack bot OAuth token (xoxb-...)
//   SLACK_NOTIFIER_CHANNEL_ID  — Slack channel ID for notifications
//   GITHUB_TOKEN               — GitHub PAT for API rate limits
//   VERCEL_API_TOKEN           — Vercel REST API token for Edge Config writes
//   EDGE_CONFIG_ID             — Edge Config store ID (ecfg_...)
//
// Optional:
//   TRACKED_GITHUB_AUTHORS     — comma-separated (default: czarcas7ic,p0mvn,greg0x,ValarDragon)
//   TRACKED_REPO_OWNERS        — comma-separated (default: ZcashFoundation,zcash,zodl-inc)
//   SLACK_MENTION_IDS          — comma-separated Slack user IDs to @-mention
//   AUTHOR_SLACK_MAP           — github:slackId pairs (default: czarcas7ic:U0A8B0NM744,...)
//   NOTIFIER_DRY_RUN           — "true" to log without posting to Slack

export interface NotifierConfig {
  trackedAuthors: string[];
  trackedOrgs: string[];
  slackBotToken: string;
  slackChannelId: string;
  slackMentionIds: string[];
  authorSlackMap: Record<string, string>;
  githubToken: string;
  vercelApiToken: string;
  edgeConfigId: string;
  reconcileIntervalMs: number;
  dryRun: boolean;
}

export function loadConfig(): NotifierConfig | { error: string } {
  const slackBotToken = process.env.SLACK_BOT_TOKEN;
  const slackChannelId = process.env.SLACK_NOTIFIER_CHANNEL_ID;
  const githubToken = process.env.GITHUB_TOKEN;
  const vercelApiToken = process.env.VERCEL_API_TOKEN;
  const edgeConfigId = process.env.EDGE_CONFIG_ID;

  const missing = [
    !slackBotToken && 'SLACK_BOT_TOKEN',
    !slackChannelId && 'SLACK_NOTIFIER_CHANNEL_ID',
    !githubToken && 'GITHUB_TOKEN',
    !vercelApiToken && 'VERCEL_API_TOKEN',
    !edgeConfigId && 'EDGE_CONFIG_ID',
  ].filter(Boolean);

  if (missing.length > 0) {
    return { error: `Missing required env vars: ${missing.join(', ')}` };
  }

  return {
    trackedAuthors: splitCsv(
      process.env.TRACKED_GITHUB_AUTHORS || 'czarcas7ic,p0mvn,greg0x,ValarDragon',
    ),
    trackedOrgs: splitCsv(
      process.env.TRACKED_REPO_OWNERS || 'ZcashFoundation,zcash,zodl-inc',
    ),
    slackBotToken: slackBotToken!,
    slackChannelId: slackChannelId!,
    slackMentionIds: splitCsv(process.env.SLACK_MENTION_IDS || ''),
    authorSlackMap: parseAuthorSlackMap(
      process.env.AUTHOR_SLACK_MAP ||
        'czarcas7ic:U0A8B0NM744,p0mvn:U0A81KAPYMR,greg0x:U0A8L9SA4QH,ValarDragon:U0A7RS10AJ3',
    ),
    githubToken: githubToken!,
    vercelApiToken: vercelApiToken!,
    edgeConfigId: edgeConfigId!,
    reconcileIntervalMs: 24 * 60 * 60 * 1000,
    dryRun: process.env.NOTIFIER_DRY_RUN === 'true',
  };
}

function splitCsv(s: string): string[] {
  return s
    .split(',')
    .map((v) => v.trim())
    .filter(Boolean);
}

function parseAuthorSlackMap(s: string): Record<string, string> {
  const map: Record<string, string> = {};
  for (const pair of splitCsv(s)) {
    const [gh, slack] = pair.split(':');
    if (gh && slack) map[gh.toLowerCase()] = slack;
  }
  return map;
}

// Simple hash of the config fields that affect Slack message rendering.
// When this changes between deploys, the poll handler auto-refreshes all
// parent messages so no manual curl is needed.
export function renderingConfigHash(cfg: NotifierConfig): string {
  const parts = [
    cfg.slackMentionIds.sort().join(','),
    Object.entries(cfg.authorSlackMap)
      .sort(([a], [b]) => a.localeCompare(b))
      .map(([k, v]) => `${k}:${v}`)
      .join(','),
    cfg.slackChannelId,
  ];
  // djb2-style hash — lightweight, no crypto dependency needed in Edge runtime.
  const str = parts.join('|');
  let hash = 5381;
  for (let i = 0; i < str.length; i++) {
    hash = ((hash << 5) + hash + str.charCodeAt(i)) | 0;
  }
  return (hash >>> 0).toString(36);
}
