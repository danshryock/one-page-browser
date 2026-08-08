// Standalone profile-menu popover: fetches profile.info and renders the
// shared menu (see profile_menu.js) immediately, always "open" — the native
// gtk::Popover hosting this page is what owns show/hide, so there's no
// local open/closed toggle state here the way the in-page dropdown needs.

async function init() {
  const profileInfo = await rpcCall("profile.info", {});
  const root = document.getElementById("menu-root");
  root.innerHTML = profileMenuHtml(profileInfo);
  wireProfileMenu(root);
  console.log(`profile_menu_loaded name=${profileInfo.name}`);
}

init().catch((err) => console.error(err));
