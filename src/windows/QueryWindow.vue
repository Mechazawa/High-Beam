<template>
  <div id="app">
    <!--suppress HtmlFormInputWithoutLabel -->
    <input ref="query" placeholder="Query..." v-model="query" class="query" @keydown="onKeypress">
    <QueryResultTable :results="sortedResults"/>
  </div>
</template>

<script>
  import { ipcRenderer } from 'electron';
  import { randomString } from '../utils/data';
  import QueryResultTable from '../components/QueryResultTable';

  export default {
    name: 'query-window',
    components: { QueryResultTable },
    data () {
      return {
        query: '',
        replyKey: null,
        results: [],
        boundReplyHandler: (...args) => this.replyHandler(...args),
        maxRows: 9,
      };
    },
    watch: {
      query (value) {
        if (this.replyKey) {
          ipcRenderer.removeListener(this.replyKey, this.boundReplyHandler);

          this.replyKey = null;
        }

        this.results.splice(0, this.results.length);

        if (value.length > 0) {
          ipcRenderer.on(this.replyKey = randomString(), this.boundReplyHandler);
          ipcRenderer.send('input:query?', this.replyKey, value);
        }
      },
      results (value) {
        ipcRenderer.send('setBounds', { height: 80 + (60 * Math.min(value.length, this.maxRows)) });
      },
    },
    mounted () {
      this.$refs.query.focus();
    },
    computed: {
      sortedResults () {
        return Array.from(this.results)
          .sort((a, b) => (b?.weight ?? 50) - (a?.weight ?? 50))
          .slice(0, this.maxRows);
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
      },
    },
  };
</script>

<style lang="scss">
  @import '../assets/variables.scss';

  #app {
    font-family: Avenir, Helvetica, Arial, sans-serif;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
    color: var(--main-font-color);
    background: var(--background-color);
  }

  .query {
    width: 100%;
    font-size: 40pt;
    padding: 4pt 8pt;

    margin: 0;
    outline: 0;
    border: 0;
    background:var(--background-color);
    color: var(--main-font-color);

    :focus {
      outline: 0;
      border: 0;
    }
  }
</style>
