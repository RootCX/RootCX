/// <reference path="./rootcx-worker.d.ts" />

serve({
  rpc: {
    ping: () => ({ pong: true }),
    echo: (params) => params,
    whoami: (_, caller) => caller,
  },
});
