/**
 * This file will automatically be loaded by vite and run in the "renderer" context.
 */

import React from 'react'
import ReactDOM from 'react-dom/client'
import QueryWindow from "../../components/QueryWindow";

ReactDOM.createRoot(document.body).render(
  <React.StrictMode>
    <QueryWindow />
  </React.StrictMode>,
)