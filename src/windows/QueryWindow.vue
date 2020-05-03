<template>
  <div id="app">
    <!--suppress HtmlFormInputWithoutLabel -->
    <input placeholder="Query..." v-model="query" class="query" @keydown="onKeypress">
    <QueryResultRow
      v-for="(result, index) in results"
      :key="result.key"
      :index="index"
      v-bind="result"/>
  </div>
</template>

<script>
  import { ipcRenderer } from 'electron';
  import { randomString } from '../utils/data';
  import QueryResultRow from '../components/QueryResultRow';

  export default {
    name: 'query-window',
    components: { QueryResultRow },
    data () {
      return {
        query: '',
        replyKey: null,
        results: [],
        boundReplyHandler: (...args) => this.replyHandler(...args),
      };
    },
    watch: {
      query (value) {
        if (this.replyKey) {
          ipcRenderer.removeListener(this.replyKey, this.boundReplyHandler);

          this.replyKey = null;
        }

        this.results = [];

        if (value.length > 0) {
          ipcRenderer.on(this.replyKey = randomString(), this.boundReplyHandler);
          ipcRenderer.send('input:query?', this.replyKey, value);
        }
      },
    },
    methods: {
      onKeypress ({ code }) {
        if (code === 'Escape') {
          window.close();
        }
      },
      replyHandler (event, rows) {
        this.results.push(...rows);

        ipcRenderer.send('setBounds', { height: 80 + (60 * this.results.length) });
      },
    },
  };
</script>

<style lang="scss">
  #app {
    font-family: Avenir, Helvetica, Arial, sans-serif;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
    color: #2c3e50;
  }

  .query {
    width: 100%;
    font-size: 40pt;
    padding: 4pt 8pt;

    margin: 0;
    outline: 0;
    border: 0;

    :focus {
      outline: 0;
      border: 0;
    }
  }
</style>
