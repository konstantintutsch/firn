FROM node:24-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# Runs as an unprivileged user, on port 8080 inside the container.
FROM caddy:2.11-alpine
COPY Caddyfile /etc/caddy/Caddyfile
COPY --from=build /app/_site /srv
# Caddy keeps a lock and an identifier here, even without certificates.
RUN chown -R 101:101 /data /config
USER 101:101
