/**
 * This file will automatically be loaded by vite and run in the "renderer" context.
 */

import React from 'react'
import ReactDOM from 'react-dom/client'
import QueryWindow from "../../components/QueryWindow";
import AutoResize from "../../components/AutoResize";

ReactDOM.createRoot(document.getElementById('app')).render(
  <React.StrictMode>
    <AutoResize>
      <QueryWindow />
    </AutoResize>
  </React.StrictMode>,
);

window.addEventListener('keydown', (event: KeyboardEvent) => {
  if (event.key === 'Escape') {
    window.close();
  }
});