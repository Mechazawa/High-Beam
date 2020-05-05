import Vue from 'vue';
import App from './QueryWindow.vue';
import { remote } from 'electron';

const { systemPreferences } = remote;

const setOSTheme = () => {
  window.localStorage.os_theme = systemPreferences.isDarkMode() ? 'dark' : 'light';

  //
  // Defined in index.html, so undefined when launching the app.
  // Will be defined for `systemPreferences.subscribeNotification` callback.
  //
  if ('__setTheme' in window) {
    window.__setTheme();
  }
};

const subscriptionId = systemPreferences.subscribeNotification(
  'AppleInterfaceThemeChangedNotification',
  setOSTheme,
);

window.onClose = () => {
  systemPreferences.unsubscribeNotification(subscriptionId);
};

setOSTheme();

Vue.config.productionTip = false;

export default new Vue({
  render: h => h(App),
}).$mount('#app');
