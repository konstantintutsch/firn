FROM node:24-alpine AS build
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# Runs as an unprivileged user, on port 8080 inside the container.
FROM nginxinc/nginx-unprivileged:1.29-alpine
COPY nginx.conf /etc/nginx/conf.d/default.conf
COPY --from=build /app/_site /usr/share/nginx/html
