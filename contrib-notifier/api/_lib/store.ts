// Durable state for the PR notifier, persisted in Vercel Edge Config.
//
// State is stored under a single Edge Config key ("notifier-state") and
// updated via the Vercel REST API.

import { get } from '@vercel/edge-config';

export interface TrackedPR {
  owner: string;
  repo: string;
  number: number;
  author: string;
  title: string;
  url: string;
  state: 'open' | 'closed' | 'merged';
  slackThreadTs: string;
  lastEventAt: string;
  discoveredAt: string;
}

export interface NotifierState {
  trackedPrs: Record<string, TrackedPR>;
  lastDiscoveryPoll: string;
  lastReconcilePoll: string;
  configHash?: string;
}

const EDGE_CONFIG_KEY = 'notifier-state';

const EMPTY_STATE: NotifierState = {
  trackedPrs: {},
  lastDiscoveryPoll: new Date(0).toISOString(),
  lastReconcilePoll: new Date(0).toISOString(),
};

export function prKey(owner: string, repo: string, num: number): string {
  return `${owner}/${repo}#${num}`;
}

export async function loadState(): Promise<NotifierState> {
  const raw = (await get(EDGE_CONFIG_KEY)) as NotifierState | null;
  return raw ?? { ...EMPTY_STATE, trackedPrs: {} };
}

export async function saveState(
  state: NotifierState,
  vercelApiToken: string,
  edgeConfigId: string,
): Promise<void> {
  const resp = await fetch(
    `https://api.vercel.com/v1/edge-config/${edgeConfigId}/items`,
    {
      method: 'PATCH',
      headers: {
        Authorization: `Bearer ${vercelApiToken}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        items: [{ operation: 'upsert', key: EDGE_CONFIG_KEY, value: state }],
      }),
    },
  );
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(`Edge Config update failed: HTTP ${resp.status} – ${text}`);
  }
}
