serve({
  ping: () => ({ pong: true }),
  echo: (params) => params,
  whoami: (_, caller) => caller,
});
