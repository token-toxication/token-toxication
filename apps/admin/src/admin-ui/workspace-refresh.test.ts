import { describe, expect, it, vi } from "vite-plus/test";

import { createWorkspaceRefresh } from "./workspace-refresh";

function deferred<T>() {
  let resolve: ((value: T) => void) | undefined;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return {
    promise,
    resolve(value: T) {
      if (!resolve) throw new Error("uninitialized promise");
      resolve(value);
    },
  };
}

describe("workspace refresh ordering", () => {
  it("does not overwrite confirmed credits permission with an older workspace", async () => {
    const older = deferred<{ allowCredits: boolean }>();
    const newer = deferred<{ allowCredits: boolean }>();
    const load = vi.fn().mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    const apply = vi.fn();
    const workspace = createWorkspaceRefresh(load, apply);
    const first = workspace.refresh();
    const second = workspace.refresh();
    newer.resolve({ allowCredits: true });
    await second;
    older.resolve({ allowCredits: false });
    await first;
    expect(apply.mock.calls).toEqual([[{ allowCredits: true }]]);
  });

  it("does not apply a pending response after cleanup", async () => {
    const pending = deferred<boolean>();
    const apply = vi.fn();
    const workspace = createWorkspaceRefresh(() => pending.promise, apply);
    const refresh = workspace.refresh();
    workspace.invalidate();
    pending.resolve(true);
    await refresh;
    expect(apply).not.toHaveBeenCalled();
  });

  it("ignores obsolete errors without hiding a current refresh failure", async () => {
    const failure = new Error("synthetic refresh failure");
    const workspace = createWorkspaceRefresh(async () => {
      throw failure;
    }, vi.fn());
    const obsolete = workspace.refresh();
    workspace.invalidate();
    await expect(obsolete).resolves.toBeUndefined();
    await expect(workspace.refresh()).rejects.toBe(failure);
  });
});
