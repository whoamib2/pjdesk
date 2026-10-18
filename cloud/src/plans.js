// Plan rules for PJDesk Cloud. The site advertises optional Cloud Sync and
// no prices, so the only plans are Free (sync off) and Cloud (sync on).

export const PLANS = ["free", "cloud"];

export function normalizePlan(plan) {
  return plan === "cloud" ? "cloud" : "free";
}

export function isPlan(plan) {
  return plan === "free" || plan === "cloud";
}

/**
 * What an account can do right now.
 * Administrators always have Cloud Sync. A Cloud grant with an expiry at or
 * before `now` is Free, and the caller should turn sync off and write an audit row.
 *
 * @param {{ plan?: string, expiresAt?: number|null, isAdmin?: boolean, now: number }} row
 * @returns {{ plan: "free"|"cloud", sync: boolean, expired: boolean }}
 */
export function effectivePlan({ plan, expiresAt, isAdmin, now }) {
  if (isAdmin) return { plan: "cloud", sync: true, expired: false };
  const named = normalizePlan(plan);
  const exp = Number(expiresAt);
  const expired = named === "cloud" && Number.isFinite(exp) && exp > 0 && exp <= now;
  if (expired) return { plan: "free", sync: false, expired: true };
  return { plan: named, sync: named === "cloud", expired: false };
}
