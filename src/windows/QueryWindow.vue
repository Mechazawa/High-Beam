<template>
  <div id="app">
    <!--suppress HtmlFormInputWithoutLabel -->
    <input ref="query" placeholder="Query..." v-model="query" class="query" @keydown="onInputKeypress">
    <div>
      <QueryResultRow
          v-for="(result, index) in sortedResults"
          :key="result.key"
          :index="index"
          v-bind="result"
          :highlight="highlighted === index"
          @click.native.meta.exact="select(index, true)"
          @click.native.exact="select(index)"
          @mouseover.native="hover(index)"/>
    </div>
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
        maxRows: 9,
        replyKey: null,
        results: [],
        highlighted: -1,
        boundReplyHandler: (...args) => this.replyHandler(...args),
        boundOnWindowKeydown: (...args) => this.onWindowKeydown(...args),
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
      results () {
        this.highlighted = -1;
        this.updateBounds();
      },
    },
    mounted () {
      this.$refs.query.focus();
      window.addEventListener('keydown', this.boundOnWindowKeydown);
      setTimeout(() => {
        this.updateBounds();
        ipcRenderer.send('center');
        ipcRenderer.send('setOpacity', 0.94);
      }, 0);
    },
    beforeDestroy () {
      window.removeEventListener('keydown', this.boundOnWindowKeydown);
    },
    computed: {
      sortedResults () {
        return Array.from(this.results)
                    .sort((a, b) => (b?.weight ?? 50) - (a?.weight ?? 50))
                    .slice(0, this.maxRows);
      },
    },
    methods: {
      onInputKeypress ({ code }) {
        if (code === 'Escape') {
          window.close();
        } else if (code === 'Tab') {
          const index = this.highlighted <= 0 ? 0 : this.highlighted;

          if (!this.sortedResults[index]) {
            return;
          }

          const { title } = this.sortedResults[index];
          const div = document.createElement('div');

          div.innerHTML = title;

          if (this.query !== div.innerText) {
            this.query = div.innerText;

            setTimeout(() => {
              const len = this.query.length * 2;

              this.$refs.query.setSelectionRange(len, len);
            }, 10);
          }
        }
      },
      replyHandler (event, rows) {
        this.results.push(...rows);
      },
      onWindowKeydown ({ code, metaKey }) {
        const digits = Object.fromEntries([123456789].map(x => [`Digit${x}`, x]));

        if (code === 'ArrowUp' && this.highlighted >= 0) {
          this.highlighted--;
        } else if (code === 'ArrowDown' && this.highlighted < this.results.length - 1) {
          this.highlighted++;
        } else if (code === 'Enter' && this.highlighted >= 0) {
          this.select(this.highlighted, metaKey);
        } else if (code === 'Enter' && this.results.length > 0) {
          this.select(0, metaKey);
        } else if (metaKey && digits.hasOwnProperty(code)) {
          this.select(digits[code]);
        }
      },
      select (index, meta = false) {
        if (!this.results[index]) {
          return;
        }

        const { key, pluginName } = this.results[index];

        ipcRenderer.send('input:select?', pluginName, key, meta);
      },
      hover (index) {
        this.highlighted = index;
      },
      updateBounds () {
        const bounds = {
          width: 800,
          height: 74 + (60 * Math.min(this.results.length, this.maxRows))
        };

        ipcRenderer.send('setBounds', bounds);
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

  body {
    background: var(--background-color);
  }

  .query {
    width: 100%;
    font-size: 40pt;
    padding: 4pt 8pt;

    margin: 0;
    outline: 0;
    border: 0;
    background: var(--background-color);
    color: var(--main-font-color);

    :focus {
      outline: 0;
      border: 0;
    }
  }
</style>
