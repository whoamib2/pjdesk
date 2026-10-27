/** Wrong account passwords. A correct password is not counted. A 429 is not counted. */
export const LOGIN_FAILS = { limit: 10, window: 15 * 60 };
/** Wrong account passwords from one address, any email. */
export const LOGIN_IP = { limit: 30, window: 15 * 60 };

/**
 * Seconds until this sliding window drops below `limit`.
 * `events` are unix timestamps. A time at or before `nowSec - window` is already outside.
 * Returns 0 when fewer than `limit` events are still inside the window.
 * The wait is the moment the oldest events still needed to stay at the limit fall out.
 */
export function retryAfterSeconds(events, nowSec, { limit, window }) {
  const cutoff = nowSec - window;
  const live = (events || [])
    .map((at) => Number(at) || 0)
    .filter((at) => at > cutoff)
    .sort((a, b) => a - b);
  if (live.length < limit) return 0;
  const at = live[live.length - limit];
  return Math.max(1, at + window - nowSec);
}

/** The longer of the email bucket and the IP bucket. 0 when neither is full. */
export function loginRetryAfter(emailEvents, ipEvents, nowSec) {
  return Math.max(
    retryAfterSeconds(emailEvents, nowSec, LOGIN_FAILS),
    retryAfterSeconds(ipEvents, nowSec, LOGIN_IP),
  );
}

/** The sentence a blocked sign-in shows. N is Retry-After rounded up to a minute. */
export function signInLimitMessage(seconds) {
  const minutes = Math.max(1, Math.ceil(Number(seconds) / 60));
  const unit = minutes === 1 ? "minute" : "minutes";
  return `Too many sign-in attempts. Try again in ${minutes} ${unit}.`;
}
