serve({
  rpc: {
    ping: () => ({ pong: true }),
    create: async (params, _caller, ctx) =>
      ctx.transaction(async (tx) => {
        const created = await tx.sql(
          "INSERT INTO items (name) VALUES ($1) RETURNING id",
          [params.name],
        );
        return { id: created.rows[0][0] };
      }),
  },
  onJob: async (payload, _caller, ctx) => {
    await ctx.sql("INSERT INTO job_log (payload) VALUES ($1::jsonb)", [payload]);
    return { done: true };
  },
});
