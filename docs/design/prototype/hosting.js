/* GitWarp hosting accounts: service table, sample accounts, status chips and the Add / Reconnect account dialog.
   Design-prototype code: each state machine is a spec for the GPUI implementation. Needs app.js, settings.js (S.dialog + builders), dialogs.js. */
(function () {
  'use strict';
  const G = window.GL, ic = G.ic, esc = G.esc, S = window.GLS;
  const H = (window.GLH = {});

  /* ---------- services ---------- */
  H.SVC = {
    github: { name: 'GitHub', mono: '', pub: 'github.com', oauth: 'device', ent: 'GitHub Enterprise Server', perms: ['repo', 'read:user', 'workflow'], tokenWhere: 'Settings → Developer settings → Personal access tokens', redirScopes: 'repo read:user workflow', appWhere: 'Settings → Developer settings → OAuth Apps (tick “Enable Device Flow”)' },
    gitlab: { name: 'GitLab', mono: 'GL', pub: 'gitlab.com', oauth: 'device', ent: 'GitLab self-managed', perms: ['api', 'read_repository', 'write_repository'], tokenWhere: 'Preferences → Access tokens', redirScopes: 'api read_repository write_repository', appWhere: 'Preferences → Applications (or Admin Area → Applications), untick “Confidential”' },
    gitea: { name: 'Gitea / Forgejo', mono: 'GT', pub: 'codeberg.org', oauth: 'redirect', ent: 'Gitea / Forgejo', perms: ['repository: read and write', 'user: read and write'], tokenWhere: 'Settings → Applications → Generate new token', redirScopes: 'read:repository write:repository read:user', appWhere: 'Settings → Applications → Create OAuth2 Application, untick “Confidential client”' },
    bitbucket: { name: 'Bitbucket', mono: 'BB', pub: 'bitbucket.org', oauth: null, ent: 'Bitbucket', perms: ['Repositories: read and write', 'Pull requests: read and write'], tokenWhere: 'Personal settings → API tokens (Bitbucket API token)' },
    azure: { name: 'Azure DevOps', mono: 'AZ', pub: 'dev.azure.com', oauth: null, ent: 'Azure DevOps Server', perms: ['Code: read and write'], tokenWhere: 'User settings → Personal access tokens' }
  };
  H.svcIcon = (svc, size) => `<span class="svcicon" aria-hidden="true">${svc === 'github' ? ic('github', size || 16) : `<span class="mono-badge">${H.SVC[svc].mono}</span>`}</span>`;
  H.avatar = (login, lg) => `<span class="av${lg ? ' lg' : ''}" aria-hidden="true">${esc((login || '?').slice(0, 1))}</span>`;
  H.isPublic = (svc, host) => H.SVC[svc].pub === host;

  /* ---------- sample accounts (persisted like the other settings) ---------- */
  H.DEFAULT = [
    { id: 'a1', svc: 'github', host: 'github.com', login: 'mira-k', label: 'mira-k', method: 'browser', status: 'ok' },
    { id: 'a2', svc: 'github', host: 'github.com', login: 'mira-at-acme', label: 'mira-at-acme', method: 'token', status: 'expiring', days: 5 },
    { id: 'a3', svc: 'gitlab', host: 'gitlab.com', login: 'mira-k', label: 'mira-k', method: 'browser', status: 'ok' },
    { id: 'a4', svc: 'gitlab', host: 'gitlab.modsen.app', login: 'yegor.cheprasov', label: 'yegor.cheprasov', method: 'token', status: 'unreachable' },
    { id: 'a5', svc: 'gitea', host: 'codeberg.org', login: 'mira-k', label: 'mira-k', method: 'token', status: 'invalid' },
    { id: 'a6', svc: 'github', host: 'git.acme.corp', login: 'mkovalenko', label: 'mkovalenko', method: 'token', status: 'checking' }
  ];
  H.accounts = () => S.get('hosting-accounts', H.DEFAULT.map((a) => Object.assign({}, a)));
  H.saveAccounts = (l) => S.set('hosting-accounts', l);
  H.CLI = [{ svc: 'github', host: 'github.com', login: 'mira-k', tool: 'gh' }, { svc: 'gitlab', host: 'gitlab.modsen.app', login: 'yegor.cheprasov', tool: 'glab' }];
  H.STAT = { ok: ['check', 'Connected', 'ok'], checking: ['fetch', 'Checking…', 'muted'], invalid: ['err', 'Token invalid or revoked', 'err'], expiring: ['warn', 'Token expires in 5 days', 'warn'], unreachable: ['err', 'Instance unreachable', 'err'], unchecked: ['warn', 'Not checked', 'muted'] };
  H.WHY = { unchecked: 'No token is stored in the system keychain for this account.', invalid: 'The service answered 401 Bad credentials.', unreachable: 'connection timed out' };
  H.statHTML = (a) => { const s = H.STAT[a.status]; const t = a.status === 'expiring' ? `Token expires in ${a.days || 5} ${(a.days || 5) === 1 ? 'day' : 'days'}` : s[1]; const why = H.WHY[a.status]; return `<span class="stat ${s[2]}"${why ? ` data-tip="${esc(why)}"` : ''}>${ic(s[0], 12, a.status === 'checking' ? 'spin' : '')}${t}</span>`; };
  H.label = (a) => a.label || a.login;
  H.fp = 'SHA256:Zk4mT9vQ1pLx7bYdW3nR0aCeHs8UfJ2oGi6tNyVq5Xc7Ab1Ld3E';
  const fmt = (s) => `${Math.floor(s / 60)}:${String(s % 60).padStart(2, '0')}`;
  const CODE = { github: 'WDJB-MJHT', gitlab: 'KQ4R-7XPD' };

  /* ---------- Add / Reconnect account dialog ----------
     H.accountDialog(win, { mode: 'add' | 'reconnect', st: {...overrides}, existing: [accounts], onDone(account) })
     Steps: 1 service · 2 instance (probe) · 3 sign-in (token | browser) · waiting (device code / redirect) · result. */
  H.accountDialog = function (win, o) {
    o = o || {};
    const existing = o.existing || H.accounts();
    const st = Object.assign({ step: 1, svc: null, inst: 'public', url: '', probe: null, probeSvc: null, trust: false, method: 'token', token: '', show: false, clientId: '', wait: null, res: null, label: '', mode: o.mode || 'add', static: false, secs: 900, login: 'mira-k', lockedHost: null, miss: [] }, o.st || {});
    const host = () => (st.inst === 'public' ? H.SVC[st.svc].pub : st.url.trim().replace(/^https?:\/\//, '').replace(/\/.*$/, ''));
    const oauthOk = () => !!H.SVC[st.svc].oauth && (st.inst === 'public' || browserOk());
    let timer = null;
    const stop = () => { clearInterval(timer); timer = null; };
    const busy = () => !!st.wait;
    const browserOk = () => st.svc === 'github' && st.inst === 'public'; /* the app's OAuth app exists for github.com only (accounts.rs browser_available) */
    if (st.method === 'browser' && !browserOk()) st.method = 'token';
    const plainHttp = () => st.inst === 'self' && /^http:\/\//i.test(st.url.trim());
    const httpWarn = () => (plainHttp() ? S.dbanner('warn', 'This address uses plain HTTP — the token would be sent unencrypted.') : '');
    const dupOnHost = () => existing.some((a) => a.host === host());
    const stepper = () => {
      if (st.mode === 'reconnect') return '';
      const n = st.step === 'res' ? 4 : st.step; const L = ['Service', 'Instance', 'Sign in'];
      return `<ol class="hstep" aria-label="Progress">${L.map((l, i) => `<li class="${i + 1 === n ? 'now' : i + 1 < n ? 'done' : ''}"${i + 1 === n ? ' aria-current="step"' : ''}><span class="n">${i + 1 < n ? ic('check', 10) : i + 1}</span>${l}</li>${i < 2 ? ic('chevR', 12) : ''}`).join('')}</ol>`;
    };
    const lockLine = () => `<div class="lockline">${H.svcIcon(st.svc)}<div style="min-width:0"><b>${H.SVC[st.svc].name}</b> <span class="muted">· ${esc(host())}${st.login && st.mode === 'reconnect' ? ' · ' + esc(st.login) : ''}</span></div>${ic('lock', 14)}</div>`;

    /* step 1 */
    const step1 = () => `<div class="optgrid" role="group" aria-label="Service type">${Object.entries(H.SVC).map(([k, v], i, arr) => `<button type="button" class="optcard${i === arr.length - 1 ? ' wide' : ''}" data-do="pick" data-v="${k}" aria-pressed="${st.svc === k}">${H.svcIcon(k)}<span class="t">${v.name}</span><span class="d">${k === 'gitea' ? 'Codeberg, Gitea and Forgejo servers' : k === 'azure' ? 'Azure DevOps Services and Server' : v.pub + ' or your own server'}</span></button>`).join('')}</div>`;

    /* step 2 */
    const probeBanner = () => {
      const h = esc(host());
      if (st.probe === 'checking') return S.dbanner('info', 'Checking ' + (st.inst === 'self' ? 'https://' : '') + host() + '…', 'Looking for the service type and API version.').replace('class="banner info"', 'class="banner info" aria-busy="true"');
      if (st.probe === 'ok') return S.dbanner('ok', st.inst === 'public' ? `${H.SVC[st.svc].name} · ${host()}` : st.svc === 'gitlab' ? 'Detected: GitLab 18.4' : st.svc === 'github' ? 'Detected: GitHub 3.14' : st.svc === 'gitea' ? 'Detected: Gitea / Forgejo 9.0' : 'Detected: ' + H.SVC[st.svc].name, st.inst === 'public' ? 'Public instance, reachable.' : 'The address is reachable.');
      if (st.probe === 'other') return `<div class="banner warn" role="status">${ic('warn', 16)}<div style="min-width:0"><div class="b-title">${h} is a ${H.SVC[st.probeSvc].name} server, not ${H.SVC[st.svc].name}</div><div>The address answered like ${H.SVC[st.probeSvc].name}. Switch the service type to keep going.</div><div style="margin-top:8px">${S.dbtn('Switch to ' + H.SVC[st.probeSvc].name, { act: 'switchSvc', kind: 'primary' })}</div></div></div>`;
      if (st.probe === 'unreach') return `<div class="banner error" role="alert">${ic('err', 16)}<div style="min-width:0"><div class="b-title">Can’t reach ${h}</div><div>Check the address, your network or VPN, or the proxy settings. connection refused (os error 111)</div><div style="margin-top:8px">${S.dbtn('Try again', { act: 'probe', icon: 'sync' })}</div></div></div>`;
      if (st.probe === 'tls') return `<div class="banner warn" role="alert">${ic('warn', 16)}<div style="min-width:0"><div class="b-title">The certificate of ${h} is not trusted</div><div>invalid peer certificate: UnknownIssuer</div><div class="field-label" style="margin:8px 0 0">SHA-256 fingerprint <span class="planned-tag">Planned</span></div><div class="fpr">${H.fp}</div><div style="margin-top:8px">${S.dbtn('Trust for this instance', { act: 'trust', kind: 'primary', icon: 'shield' })} <span class="planned-tag">Planned</span></div></div></div>`;
      if (st.probe === 'notrec') return `<div class="banner error" role="alert">${ic('err', 16)}<div style="min-width:0"><div class="b-title">No supported service found at ${h}</div><div>Check that this is the web address of the service, without a path.</div></div></div>`;
      return '';
    };
    const step2 = () => {
      const pub = H.SVC[st.svc].pub, urlOk = /^https?:\/\/[^\s/]+\.[^\s/]+/.test(st.url.trim()) || /^[^\s/]+\.[^\s/]+/.test(st.url.trim());
      const showErr = st.inst === 'self' && st.url.trim() && !urlOk;
      return `<div class="od-stack" style="--od-gap:8px" role="radiogroup" aria-label="Instance">
        <button type="button" class="optcard wide" role="radio" data-do="inst" data-v="public" aria-checked="${st.inst === 'public'}">${H.svcIcon(st.svc)}<span class="t">${pub}</span><span class="d">Public instance</span></button>
        <button type="button" class="optcard wide" role="radio" data-do="inst" data-v="self" aria-checked="${st.inst === 'self'}">${ic('org', 16)}<span class="t">Self-hosted</span><span class="d">${esc(H.SVC[st.svc].ent)} on your own domain or intranet</span></button></div>
        ${st.inst === 'self' ? S.dfield('Instance URL', 'url', S.din('url', st.url, 'https://git.example.com', { mono: 1 }), 'The address you open in the browser, without a path.', showErr ? 'Enter a web address like https://git.example.com.' : '') : ''}
        ${httpWarn()}${probeBanner()}`;
    };

    /* step 3 */
    const resBanner = () => {
      const r = st.res; if (!r || r.k === 'ok') return '';
      const n = H.SVC[st.svc].name, h = esc(host());
      if (r.k === 'wrong') return S.dbanner('error', 'That token was rejected', `${n} answered 401 Bad credentials. Check that you pasted the whole token and that it has not been revoked or expired.`);
      if (r.k === 'perms') return `<div class="banner error" role="alert">${ic('err', 16)}<div style="min-width:0"><div class="b-title">The token is missing permissions</div><div>${n} accepted the token, but it cannot do everything GitWarp needs. Create a new token that includes:</div><ul class="perms">${(st.miss.length ? st.miss : H.SVC[st.svc].perms.slice(-2)).map((p) => `<li class="miss">${esc(p)}</li>`).join('')}</ul></div></div>`;
      if (r.k === 'denied') return `<div class="banner warn" role="alert">${ic('warn', 16)}<div style="min-width:0"><div class="b-title">Access was denied in the browser</div><div>You declined the request, or the account is not allowed to authorise apps. Nothing was saved.</div><div style="margin-top:8px">${S.dbtn('Try again', { act: 'startWait', icon: 'sync' })}</div></div></div>`;
      if (r.k === 'other') return S.dbanner('error', 'Could not check the token', 'unexpected response from the service (HTTP 500)');
      if (r.k === 'expired') return `<div class="banner warn" role="alert">${ic('warn', 16)}<div style="min-width:0"><div class="b-title">The code expired</div><div>Codes are valid for 15 minutes. Start again to get a new one.</div><div style="margin-top:8px">${S.dbtn('Restart', { act: 'startWait', kind: 'primary', icon: 'sync' })}</div></div></div>`;
      if (r.k === 'net') return `<div class="banner error" role="alert">${ic('err', 16)}<div style="min-width:0"><div class="b-title">Can’t reach ${h}</div><div>Check your network connection and try again. request timed out</div><div style="margin-top:8px">${S.dbtn('Try again', { act: 'submit', icon: 'sync' })}</div></div></div>`;
      return '';
    };
    const step3 = () => {
      const v = H.SVC[st.svc], canBrowser = browserOk(), self = st.inst === 'self';
      const tokenCard = `<button type="button" class="optcard wide" role="radio" data-do="method" data-v="token" aria-checked="${st.method === 'token'}">${ic('key', 16)}<span class="t">Token</span><span class="d">Paste a personal access token. Works everywhere.</span></button>`;
      /* Browser sign-in is selectable for github.com only; other services keep it drawn disabled (accounts.rs:1693). */
      const brCard = canBrowser
        ? `<button type="button" class="optcard wide" role="radio" data-do="method" data-v="browser" aria-checked="${st.method === 'browser'}">${ic('browser', 16)}<span class="t">Sign in with browser</span><span class="d">Approve GitWarp in your browser, no token to copy.</span></button>`
        : `<div class="optcard wide" role="radio" aria-checked="false" aria-disabled="true" aria-describedby="nb-h"${v.oauth ? ' data-tip="Coming soon"' : ''}>${ic('browser', 16)}<span class="t">Sign in with browser</span><span class="d" id="nb-h">${v.oauth ? (self ? 'Uses an OAuth app registered on this instance.' : 'Approve GitWarp in your browser, no token to copy.') : `Not available for ${v.name}: it has no sign-in flow for desktop apps. Use a token.`}</span></div>`;
      let panel = '';
      if (st.method === 'token' || !canBrowser) {
        panel = `${S.dfield('Access token', 'token', `<div class="tokrow"><input class="input masked" id="token" data-f="token" type="${st.show ? 'text' : 'password'}" value="${esc(st.token)}" placeholder="Paste the token here" autocomplete="off" spellcheck="false" aria-describedby="perm-h"><button type="button" class="btn btn-icon" data-do="show" aria-label="${st.show ? 'Hide token' : 'Show token'}" data-tip="${st.show ? 'Hide token' : 'Show token'}" aria-pressed="${st.show}">${ic(st.show ? 'eyeClosed' : 'eye', 14)}</button></div>`, '')}
          <div class="od-row" style="align-items:flex-start;gap:12px"><div id="perm-h" style="flex:1;min-width:0"><div class="field-label" style="margin:0">Required permissions${st.svc === 'github' ? ' (classic token scopes)' : ''}</div><ul class="perms">${v.perms.map((p) => `<li>${esc(p)}</li>`).join('')}</ul><div class="field-help" style="margin-top:6px">${esc(v.tokenWhere)}</div></div>${S.dbtn('Create a token', { act: 'createTok', icon: 'external' })}</div>`;
      } else {
        panel = `<div class="field-help">GitWarp opens ${esc(host())} in your browser, you approve access, and the token is stored in the system keychain.${v.oauth === 'device' ? ' You will enter a short code there.' : ''}</div>`;
      }
      return `${st.mode === 'reconnect' ? lockLine() : ''}${httpWarn()}${st.storeErr ? S.dbanner('error', 'The system keychain is not available', st.storeErr + '. GitWarp does not store tokens anywhere else.') : ''}${resBanner()}
        <div class="od-stack" style="--od-gap:8px" role="radiogroup" aria-label="Sign-in method">${tokenCard}${brCard}</div>${panel}`;
    };

    /* waiting */
    const waitView = () => {
      const h = esc(host()), plan = browserOk() ? '' : '<div><span class="planned-tag">Planned</span> <span class="muted" style="font-size:var(--fs-sm)">The app signs in with the browser on github.com only.</span></div>';
      if (st.wait === 'device') {
        const url = h + (st.svc === 'github' ? '/login/device' : '/oauth/device'), code = CODE[st.svc] || 'WDJB-MJHT';
        return `${plan}${S.dbanner('info', 'Approve GitWarp in your browser', 'Enter the code below on ' + host() + ' and confirm the permissions.')}
          <div class="devcode"><span class="code" aria-label="Device code ${code.split('').join(' ')}">${code}</span><button type="button" class="btn" data-do="copyCode">${ic('copy', 14)}Copy code</button></div>
          <div class="od-row">${S.dbtn('Open browser', { act: 'openBrowser', kind: 'primary', icon: 'external', af: 1, def: 1 })}<span class="muted" style="font-size:var(--fs-sm)">or go to <span class="mono" style="user-select:all">https://${url}</span></span></div>
          <div class="waitline" role="status">${ic('fetch', 14, 'spin')}<span>Waiting for approval… code expires in ${fmt(st.secs)}</span></div>`;
      }
      return `${plan}${S.dbanner('info', 'Continue in your browser', 'GitWarp opened ' + host() + '. Sign in there and approve the request, then come back.')}
        <div class="od-row">${S.dbtn('Open again', { act: 'openBrowser', icon: 'external', af: 1 })}<span class="muted" style="font-size:var(--fs-sm)">GitWarp listens on http://127.0.0.1 for the answer.</span></div>
        <div class="waitline" role="status">${ic('fetch', 14, 'spin')}<span>Waiting for ${h}…</span></div>`;
    };

    /* result: connected */
    const doneView = () => {
      const dup = dupOnHost() && st.mode !== 'reconnect';
      return `<div class="donecard">${H.avatar(st.login, 1)}<div style="min-width:0"><div class="t">Connected as ${esc(st.login)}</div><div class="d">${esc(H.SVC[st.svc].name)} · ${esc(host())} · ${st.method === 'browser' ? 'browser sign-in' : 'token'}</div></div></div>${st.saveErr ? S.dbanner('error', 'The token could not be saved', st.saveErr) : ''}
        ${dup ? S.dfield('Label <span class="req" aria-hidden="true">*</span>', 'label', S.din('label', st.label, 'e.g. Personal, Acme work'), `You already have an account on ${esc(host())}. A label tells them apart in clone, push and pull-request menus.`, st.labelErr ? 'Enter a label for this account.' : '') : S.dfield('Label (optional)', 'label', S.din('label', st.label, st.login), 'Defaults to the login name.')}`;
    };

    const spec = {
      st, width: 580, busy,
      view: (s) => {
        const reco = s.mode === 'reconnect';
        const title = reco ? 'Reconnect account' : 'Add account';
        let body, foot;
        if (s.step === 'res') { body = doneView(); foot = S.dbtn('Done', { kind: 'primary', act: 'done', def: 1 }); }
        else if (s.wait) { body = waitView(); foot = S.dbtn('Cancel', { act: 'cancelWait' }); }
        else if (s.step === 1) { body = step1(); foot = S.dbtn('Cancel') + S.dbtn('Continue', { kind: 'primary', act: 'next', dis: !s.svc, def: 1 }); }
        else if (s.step === 2) {
          const pub = s.inst === 'public', urlOk = /^(https?:\/\/)?[^\s/]+\.[^\s/]+/.test(s.url.trim());
          const label = s.probe === 'ok' ? 'Continue' : s.probe === 'checking' ? 'Checking…' : pub ? 'Continue' : 'Check address';
          foot = S.dbtn('Back', { act: 'back' }) + S.dbtn(label, { kind: 'primary', act: 'next', dis: s.probe === 'checking' || (!pub && !urlOk) || s.probe === 'tls' || s.probe === 'other' || s.probe === 'notrec', def: 1 }); body = step2();
        } else {
          const tokenMode = s.method === 'token' || !(s.svc === 'github' && s.inst === 'public');
          const can = tokenMode ? s.token.trim().length > 0 : oauthOk();
          foot = (reco ? S.dbtn('Cancel') : S.dbtn('Back', { act: 'back' })) + S.dbtn(tokenMode ? (reco ? 'Reconnect' : 'Connect') : 'Get code', { kind: 'primary', act: tokenMode ? 'submit' : 'startWait', dis: !can || s.verifying, def: 1 });
          body = step3();
        }
        return { title, icon: 'key', body: `<div class="col10">${stepper()}${body}</div>`, foot };
      },
      onInput: (s, f) => { if (f === 'url') { s.probe = null; } if (f === 'token') { s.res = null; } },
      actions: {
        pick: (s, api, d) => { s.svc = d.dataset.v; s.step = 2; s.inst = 'public'; s.probe = 'ok'; s.url = ''; s.method = s.svc === 'github' ? 'browser' : 'token'; },
        inst: (s, api, d) => { s.inst = d.dataset.v; s.probe = s.inst === 'public' ? 'ok' : null; s.method = s.svc === 'github' && s.inst === 'public' ? 'browser' : 'token'; },
        method: (s, api, d) => { s.method = d.dataset.v; s.res = null; },
        show: (s) => { s.show = !s.show; },
        createTok: (s, api) => api.toast('Opened ' + host() + ' in your browser to create a token', 'info'),
        copyCode: (s, api) => { try { navigator.clipboard.writeText(CODE[s.svc] || 'WDJB-MJHT'); } catch (e) {} api.toast('Code copied', 'ok'); },
        openBrowser: (s, api) => api.toast('Opened ' + host() + ' in your browser', 'info'),
        switchSvc: (s) => { s.svc = s.probeSvc; s.probe = 'ok'; s.method = 'token'; },
        trust: (s) => { s.trust = true; s.probe = 'ok'; },
        back: (s) => { s.res = null; s.step = Math.max(1, s.step - 1); },
        next: (s, api) => {
          if (s.step === 1) { if (!s.svc) return; s.step = 2; return; }
          if (s.step === 2) {
            if (s.probe === 'ok') { s.step = 3; return; }
            return spec.actions.probe(s, api);
          }
        },
        probe: (s, api) => {
          s.probe = 'checking'; const h = host().toLowerCase();
          setTimeout(() => {
            if (!api.el.isConnected) return;
            s.probe = /nosvc|unknownsvc/.test(h) ? 'notrec' : /down|unreach|offline/.test(h) ? 'unreach' : /tls|selfsigned|internal/.test(h) ? 'tls' : (s.svc === 'github' && /gitlab/.test(h)) ? 'other' : (s.svc === 'gitlab' && /gitea|forgejo|codeberg/.test(h)) ? 'other' : 'ok';
            if (s.probe === 'other') s.probeSvc = /gitlab/.test(h) ? 'gitlab' : 'gitea';
            api.redraw();
          }, 700);
        },
        submit: (s, api) => {
          s.res = null; s.verifying = true; const t = s.token.trim().toLowerCase();
          setTimeout(() => {
            if (!api.el.isConnected) return; s.verifying = false;
            if (t.length < 8 || /bad|nope/.test(t)) s.res = { k: 'wrong' };
            else if (/noperm|scope/.test(t)) { s.res = { k: 'perms' }; s.miss = H.SVC[s.svc].perms.slice(-2); }
            else if (/net|timeout/.test(t)) s.res = { k: 'net' };
            else { s.res = { k: 'ok' }; s.step = 'res'; }
            api.redraw();
          }, 800);
        },
        startWait: (s, api) => {
          s.res = null; s.wait = H.SVC[s.svc].oauth; s.secs = 900;
          stop(); if (s.static) return;
          timer = setInterval(() => { if (!api.el.isConnected) { stop(); return; } s.secs = Math.max(0, s.secs - 1); if (s.secs % 60 === 0 || s.secs < 5) api.redraw(); if (s.secs === 0) { stop(); s.wait = null; s.res = { k: 'expired' }; api.redraw(); } }, 1000);
          setTimeout(() => { if (!api.el.isConnected || !s.wait) return; stop(); s.wait = null; s.res = { k: 'ok' }; s.step = 'res'; api.redraw(); }, 5000);
        },
        cancelWait: (s) => { stop(); s.wait = null; s.res = null; },
        done: (s, api) => {
          const dup = dupOnHost() && s.mode !== 'reconnect';
          if (dup && !s.label.trim()) { s.labelErr = true; return; }
          stop(); const acc = { id: 'a' + Date.now(), svc: s.svc, host: host(), login: s.login, label: s.label.trim() || s.login, method: s.method === 'browser' && H.SVC[s.svc].oauth ? 'browser' : 'token', status: 'ok' };
          api.close(); if (o.onDone) o.onDone(acc, s); return false;
        }
      }
    };
    const api = S.dialog(win, spec);
    return api;
  };
})();
