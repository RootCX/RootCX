// Run with playwright-cli run-code; previews: simple/auth/agent/cli on ports 5891–5894.
// All Core requests are intercepted; no live accounts or records are changed.
async page => {
  const check = (condition, message) => { if (!condition) throw new Error(message); };
  const results = [];
  const pageErrors = [];
  page.on('pageerror', error => pageErrors.push(error.message));
  await page.addInitScript(() => localStorage.clear());
  let authenticated = false;
  let providers = [];
  let ssoUrl = '';
  const user = { id: 'audit-user', email: 'audit@example.test', displayName: 'Audit', createdAt: '2026-09-17T00:00:00Z' };
  await page.unrouteAll();
  await page.route('**/api/v1/**', async route => {
    const href = route.request().url();
    const pathname = '/' + href.split('?')[0].split('/').slice(3).join('/');
    const respond = (body, status = 200) => route.fulfill({ status, json: body });
    if (pathname === '/api/v1/auth/mode') return respond({ authRequired: true, magicLinkEnabled: false, providers });
    if (pathname === '/api/v1/auth/me') return respond(authenticated ? user : { error: 'Unauthorized' }, authenticated ? 200 : 401);
    if (pathname === '/api/v1/auth/logout') { authenticated = false; return respond({}); }
    if (pathname.endsWith('/authorize')) {
      ssoUrl = href;
      return route.fulfill({ contentType: 'text/html', body: '<h1>Intercepted SSO navigation</h1>' });
    }
    await route.abort();
    throw new Error('Unexpected API call: ' + pathname);
  });
  async function revealSignOut() {
    await page.waitForFunction(() => document.querySelector('[data-slot=sidebar-trigger]') || document.querySelector('button[aria-label="Sign out"]'));
    if (!await page.getByRole('button', { name: 'Sign out', exact: true }).isVisible()) {
      await page.locator('[data-slot=sidebar-trigger]').click();
    }
    await page.getByRole('button', { name: 'Sign out', exact: true }).waitFor();
  }
  for (const [index, variant] of ['simple', 'auth', 'agent', 'cli'].entries()) {
    const url = `http://127.0.0.1:${5891 + index}`;
    authenticated = false;
    providers = [{ id: 'audit-sso', displayName: 'Audit SSO' }, { id: 'other', displayName: 'Other SSO' }];
    await page.setViewportSize({ width: 1365, height: 900 });
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.goto(url);
    await page.getByRole('heading', { level: 1 }).waitFor();
    await page.evaluate(() => document.fonts.ready);
    const theme = await page.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      const body = getComputedStyle(document.body);
      return { primary: root.getPropertyValue('--primary').trim(), action: root.getPropertyValue('--action').trim(), background: body.backgroundColor, font: body.fontFamily, scheme: root.colorScheme };
    });
    check(theme.primary === '#009eff' && theme.action === '#006eaf', variant + ': incorrect theme tokens');
    check(theme.background === 'rgb(255, 255, 255)' && theme.scheme === 'light', variant + ': theme must stay light');
    check(theme.font.includes('Inter Variable'), variant + ': missing Inter');
    if (variant === 'simple') { results.push({ variant, theme, passed: true }); continue; }
    const sso = page.getByRole('button', { name: 'Continue with Audit SSO' });
    await sso.waitFor();
    check(await sso.getAttribute('data-slot') === 'button', variant + ': SSO button is not RootCX UI');
    await page.setViewportSize({ width: 390, height: 844 });
    check(await sso.evaluate(el => el.getBoundingClientRect().height) >= 44, variant + ': mobile controls too small');
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), variant + ': horizontal overflow');
    ssoUrl = '';
    await sso.click();
    await page.getByText('Intercepted SSO navigation').waitFor();
    check(ssoUrl.includes('/auth/oidc/audit-sso/authorize?'), variant + ': incorrect selected provider');

    // Returning without a session must offer a retry, never a redirect loop.
    providers = [{ id: 'audit-sso', displayName: 'Audit SSO' }];
    await page.goto(url);
    await page.getByText('Sign-in did not complete. Please try again.').waitFor();
    await page.getByRole('button', { name: 'Continue with Audit SSO' }).waitFor();

    authenticated = true;
    await page.reload();
    await revealSignOut();
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.getByRole('button', { name: 'Continue with Audit SSO' }).waitFor();
    authenticated = false;
    await page.reload();
    await page.getByRole('button', { name: 'Continue with Audit SSO' }).waitFor();

    // A fresh tab/session automatically uses the only provider.
    await page.evaluate(() => sessionStorage.clear());
    ssoUrl = '';
    await page.goto(url + '/?view=pending#details');
    await page.getByText('Intercepted SSO navigation').waitFor();
    const destination = await page.evaluate(href => new URL(href).searchParams.get('redirect_uri'), ssoUrl);
    check(destination === url + '/?view=pending#details', variant + ': lost requested page');

    providers = [];
    await page.goto(url);
    await page.getByText('No sign-in provider is configured. Contact your workspace administrator.').waitFor();
    results.push({ variant, theme, mobile: true, logout: true, recovery: true, sso: true, noLoginMethods: true });
  }
  check(pageErrors.length === 0, JSON.stringify(pageErrors));
  return results;
}
