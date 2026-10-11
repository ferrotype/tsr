// Observe only this Node process's children. Cleanup stays outside task timers.
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";

export function trackChildren() {
    const spawn = childProcess.spawn;
    const children = new Map();
    const counts = { spawned: 0, reaped: 0, peak: 0 };
    childProcess.spawn = function (...args) {
        const child = spawn.apply(this, args);
        const closed = new Promise(resolve => child.once("close", () => {
            children.delete(child);
            counts.reaped++;
            resolve();
        }));
        children.set(child, closed);
        counts.spawned++;
        counts.peak = Math.max(counts.peak, children.size);
        return child;
    };
    syncBuiltinESMExports();
    return {
        async drain() {
            if (!children.size) return;
            // The sync client unrefs its child; keep Node alive while reaping it.
            for (const child of children.keys()) child.ref();
            let timer;
            try {
                await Promise.race([
                    Promise.all(children.values()),
                    new Promise((_, reject) => {
                        timer = setTimeout(() => reject(new Error("API server did not exit after teardown")), 10000);
                    }),
                ]);
            } finally {
                clearTimeout(timer);
            }
        },
        async dispose() {
            for (const child of children.keys()) {
                child.stdin?.destroy();
                child.stdout?.destroy();
                if (child.pid) child.kill("SIGKILL");
            }
            try { await this.drain(); }
            finally {
                childProcess.spawn = spawn;
                syncBuiltinESMExports();
            }
        },
        report() { return { ...counts, remaining: children.size }; },
    };
}
