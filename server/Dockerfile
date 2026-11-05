# PeetMail's sync server. Build from the repository root (the pnpm workspace's lockfile is there):
#   docker build -f server/Dockerfile -t peetmail-sync .
# Run (see server/README.md for the settings):
#   docker run -d -p 3000:3000 -v peetmail-sync:/data -e ORIGIN=https://sync.example.com -e ALLOW_REGISTER=@example.com peetmail-sync

FROM node:24-bookworm-slim AS build
WORKDIR /src
RUN corepack enable
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
COPY server/package.json server/
RUN pnpm install --frozen-lockfile --filter peetmail-sync
COPY server/ server/
RUN pnpm --filter peetmail-sync build \
 && pnpm --filter peetmail-sync deploy --prod --legacy /app \
 && cp -r server/build server/drizzle /app/

FROM node:24-bookworm-slim
ENV NODE_ENV=production \
    PORT=3000 \
    DATABASE_PATH=/data/sync.db \
    MIGRATIONS_DIR=/app/drizzle \
    BODY_SIZE_LIMIT=4M
WORKDIR /app
COPY --from=build /app /app
RUN mkdir -p /data && chown node:node /data
USER node
VOLUME /data
EXPOSE 3000
HEALTHCHECK --interval=1m --timeout=5s CMD node -e "fetch('http://127.0.0.1:'+process.env.PORT+'/v1/health').then(r=>process.exit(r.ok?0:1),()=>process.exit(1))"
CMD ["node", "build"]
