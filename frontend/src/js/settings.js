import { requireAccount, setUpSettings, setUpSync } from "/js/account.js";

await requireAccount();
setUpSettings();
setUpSync(() => {});
