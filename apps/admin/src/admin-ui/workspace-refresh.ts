/** Apply only the newest workspace read, including reads begun after mutations. */
export function createWorkspaceRefresh<T>(load: () => Promise<T>, apply: (workspace: T) => void) {
  let generation = 0;
  return {
    async refresh() {
      const current = ++generation;
      try {
        const workspace = await load();
        if (current === generation) apply(workspace);
      } catch (error) {
        if (current === generation) throw error;
      }
    },
    invalidate() {
      generation += 1;
    },
  };
}
