import type { AnalyticsRange, AnalyticsRankBy, AnalyticsResponse } from "../types";

export type AnalyticsSelection = {
  range: AnalyticsRange;
  model?: string;
  accountId?: string;
  rankBy: AnalyticsRankBy;
};

export const selectionKey = (query: AnalyticsSelection) =>
  JSON.stringify([query.range, query.model ?? null, query.accountId ?? null, query.rankBy]);

export type AnalyticsSnapshot = {
  key: string;
  data: AnalyticsResponse | null;
  error: boolean;
  pending: boolean;
};

/** One query context, one in-flight refresh. Late responses never cross contexts. */
export function createAnalyticsLoader(
  query: AnalyticsSelection,
  load: (query: AnalyticsSelection, signal: AbortSignal) => Promise<AnalyticsResponse>,
  changed: (snapshot: AnalyticsSnapshot) => void,
) {
  const controller = new AbortController();
  let snapshot: AnalyticsSnapshot = {
    key: selectionKey(query),
    data: null,
    error: false,
    pending: false,
  };
  let disposed = false;
  return {
    async refresh() {
      if (disposed || snapshot.pending) return;
      snapshot = { ...snapshot, pending: true };
      changed(snapshot);
      try {
        const data = await load(query, controller.signal);
        if (disposed) return;
        snapshot = { ...snapshot, data, error: false, pending: false };
      } catch {
        if (disposed) return;
        snapshot = { ...snapshot, error: true, pending: false };
      }
      changed(snapshot);
    },
    dispose() {
      disposed = true;
      controller.abort();
    },
  };
}

export function ratio(numerator: number, denominator: number) {
  return denominator > 0 ? numerator / denominator : null;
}
