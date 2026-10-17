(() => {
  const header = document.querySelector('.site-header, .landing-nav');
  const nav = header?.querySelector('.main-nav');
  if (!header || !nav || header.querySelector('.mobile-nav-toggle')) return;

  const button = document.createElement('button');
  button.className = 'mobile-nav-toggle';
  button.type = 'button';
  button.setAttribute('aria-label', 'Open navigation menu');
  button.setAttribute('aria-expanded', 'false');
  button.setAttribute('aria-controls', 'mobile-main-nav');
  button.innerHTML = '<span></span><span></span><span></span>';
  nav.id = 'mobile-main-nav';
  header.insertBefore(button, nav);

  const close = () => {
    nav.classList.remove('mobile-nav-open');
    button.classList.remove('is-open');
    button.setAttribute('aria-expanded', 'false');
    button.setAttribute('aria-label', 'Open navigation menu');
  };
  button.addEventListener('click', () => {
    const open = !nav.classList.contains('mobile-nav-open');
    nav.classList.toggle('mobile-nav-open', open);
    button.classList.toggle('is-open', open);
    button.setAttribute('aria-expanded', String(open));
    button.setAttribute('aria-label', open ? 'Close navigation menu' : 'Open navigation menu');
  });
  nav.addEventListener('click', event => {
    if (event.target.closest('a')) close();
  });
  document.addEventListener('click', event => {
    if (!header.contains(event.target)) close();
  });
  document.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      close();
      button.focus();
    }
  });
  const media = window.matchMedia('(min-width: 761px)');
  media.addEventListener?.('change', event => { if (event.matches) close(); });
})();