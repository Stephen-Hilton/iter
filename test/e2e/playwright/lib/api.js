// A tiny iter_data API client for the Playwright suite (Node 18+ fetch).
// Every call throws on a non-2xx answer unless `allow` lists the status.
'use strict';

class Api {
  constructor(base, token) {
    this.base = base.replace(/\/$/, '');
    this.token = token || null;
  }
  as(token) { return new Api(this.base, token); }
  async call(method, path, body, allow) {
    const r = await fetch(this.base + path, {
      method,
      headers: Object.assign({ 'content-type': 'application/json' }, this.token ? { authorization: 'Bearer ' + this.token } : {}),
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await r.text();
    let json = null;
    try { json = text ? JSON.parse(text) : null; } catch (e) { json = text; }
    if (!r.ok && !(allow || []).includes(r.status)) {
      throw new Error(`${method} ${path} -> ${r.status}: ${typeof json === 'string' ? json : JSON.stringify(json)}`);
    }
    return { status: r.status, body: json };
  }
  async get(p) { return (await this.call('GET', p)).body; }
  async post(p, b) { return (await this.call('POST', p, b === undefined ? {} : b)).body; }
  async put(p, b) { return (await this.call('PUT', p, b)).body; }
  async patch(p, b) { return (await this.call('PATCH', p, b)).body; }
  async del(p, b) { return (await this.call('DELETE', p, b)).body; }

  static async login(base, user, password) {
    const a = new Api(base);
    const r = await a.post('/auth/login', { user, password });
    return { api: a.as(r.token), token: r.token, role: r.role, user: r.user };
  }

  // ---- project graph helpers
  graph(p) { return this.get(`/api/projects/${encodeURIComponent(p)}/graph`); }
  async nodeByName(p, name) {
    const g = await this.graph(p);
    return g.nodes.find((n) => n.name === name) || null;
  }
  async edges(p) { return (await this.graph(p)).edges; }
}

module.exports = { Api };
