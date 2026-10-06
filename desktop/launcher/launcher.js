const api = window.amorisDesktop;
const button = document.querySelector('#open');
api.getIcon().then(url => { document.querySelector('.brand').src = url; });
api.onLauncherError(message => { document.querySelector('#error').textContent = message; });
if (api.platform !== 'darwin') button.querySelector('span').textContent = 'Ctrl O';
async function open(action) {
  button.disabled = true;
  document.querySelector('#error').textContent = '';
  try { await action(); }
  catch { document.querySelector('#error').textContent = 'The project could not be opened. Choose another folder or try again.'; }
  finally { button.disabled = false; }
}
button.addEventListener('click', () => open(() => api.openProject()));
api.listRecent().then(projects => {
  if (!projects.length) return;
  const container = document.querySelector('#recent');
  container.replaceChildren();
  for (const project of projects) {
    const row = document.createElement('button');
    row.className = 'recent';
    const name = document.createElement('strong'); name.textContent = project.name;
    const location = document.createElement('small'); location.textContent = project.path;
    row.append(name, location);
    row.addEventListener('click', () => open(() => api.openRecent(project.index)));
    container.append(row);
  }
});
