// GitHub public API helpers for PR discovery and event fetching.
//
// All reads use the GitHub REST API with a PAT for rate-limit headroom
// (5 000 req/hour authenticated vs 60 unauthenticated).

// ---------- Response types ----------

export interface SearchPR {
  id: number;
  number: number;
  title: string;
  html_url: string;
  state: string;
  pull_request?: { html_url: string; merged_at: string | null };
  user: { login: string };
  repository_url: string;
  created_at: string;
  updated_at: string;
}

export interface IssueComment {
  id: number;
  html_url: string;
  body: string;
  user: { login: string };
  created_at: string;
}

export interface ReviewComment {
  id: number;
  html_url: string;
  body: string;
  user: { login: string };
  created_at: string;
}

export interface Review {
  id: number;
  html_url: string;
  body: string;
  state: string;
  user: { login: string };
  submitted_at: string;
}

// ---------- Helpers ----------

async function gh(url: string, token: string): Promise<Response> {
  return fetch(url, {
    headers: {
      Accept: 'application/vnd.github+json',
      Authorization: `Bearer ${token}`,
      'X-GitHub-Api-Version': '2022-11-28',
    },
  });
}

export function parseRepoFromUrl(repoUrl: string): {
  owner: string;
  repo: string;
} {
  const parts = repoUrl.split('/');
  return { owner: parts[parts.length - 2], repo: parts[parts.length - 1] };
}

// ---------- Discovery ----------

// One search per author with all orgs OR'd.
// GitHub search treats duplicate qualifiers of the same type as OR.
export async function searchOpenPRs(
  author: string,
  orgs: string[],
  token: string,
): Promise<SearchPR[]> {
  const parts = [
    'type:pr',
    'state:open',
    `author:${author}`,
    ...orgs.map((o) => `org:${o}`),
  ];
  const q = encodeURIComponent(parts.join(' '));
  const url = `https://api.github.com/search/issues?q=${q}&per_page=100&sort=updated&order=desc`;

  const resp = await gh(url, token);
  if (!resp.ok) {
    const text = await resp.text();
    throw new Error(
      `GitHub search failed for ${author}: ${resp.status} – ${text}`,
    );
  }
  const data = (await resp.json()) as { items: SearchPR[] };
  return data.items;
}

// ---------- Event fetching ----------

export async function fetchIssueComments(
  owner: string,
  repo: string,
  num: number,
  since: string,
  token: string,
): Promise<IssueComment[]> {
  const url = `https://api.github.com/repos/${owner}/${repo}/issues/${num}/comments?since=${encodeURIComponent(since)}&per_page=100`;
  const resp = await gh(url, token);
  if (!resp.ok) return [];
  return (await resp.json()) as IssueComment[];
}

export async function fetchReviewComments(
  owner: string,
  repo: string,
  num: number,
  since: string,
  token: string,
): Promise<ReviewComment[]> {
  const url = `https://api.github.com/repos/${owner}/${repo}/pulls/${num}/comments?since=${encodeURIComponent(since)}&per_page=100`;
  const resp = await gh(url, token);
  if (!resp.ok) return [];
  return (await resp.json()) as ReviewComment[];
}

export async function fetchReviews(
  owner: string,
  repo: string,
  num: number,
  token: string,
): Promise<Review[]> {
  const url = `https://api.github.com/repos/${owner}/${repo}/pulls/${num}/reviews?per_page=100`;
  const resp = await gh(url, token);
  if (!resp.ok) return [];
  return (await resp.json()) as Review[];
}

// ---------- PR state ----------

export async function fetchPRState(
  owner: string,
  repo: string,
  num: number,
  token: string,
): Promise<{ state: 'open' | 'closed' | 'merged'; title: string } | null> {
  const url = `https://api.github.com/repos/${owner}/${repo}/pulls/${num}`;
  const resp = await gh(url, token);
  if (!resp.ok) return null;
  const pr = (await resp.json()) as {
    merged_at: string | null;
    state: string;
    title: string;
  };
  const state: 'open' | 'closed' | 'merged' = pr.merged_at
    ? 'merged'
    : pr.state === 'closed'
      ? 'closed'
      : 'open';
  return { state, title: pr.title };
}
