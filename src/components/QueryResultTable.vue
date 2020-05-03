<template>
  <div>
    <QueryResultRow
        v-for="(result, index) in results"
        :key="result.key"
        :index="index"
        v-bind="result"
        :highlight="highlighted === index"
        @click="select(index)"/>
  </div>
</template>

<script>
  import QueryResultRow from './QueryResultRow';
  import { ipcRenderer } from 'electron';

  export default {
    name: 'query-result-table',
    components: { QueryResultRow },
    props: {
      results: {
        type: Array,
        required: true,
      },
    },
    data () {
      return {
        highlighted: -1,
        boundOnKeydown: (...args) => this.onKeydown(...args),
      };
    },
    watch: {
      results () {
        this.highlighted = -1;
      },
    },
    mounted () {
      window.addEventListener('keydown', this.boundOnKeydown);
    },
    beforeDestroy () {
      window.removeEventListener('keydown', this.boundOnKeydown);
    },
    methods: {
      onKeydown ({ code, metaKey }) {
        const digits = Object.fromEntries([123456789].map(x => [`Digit${x}`, x]));

        if (code === 'ArrowUp' && this.highlighted >= 0) {
          this.highlighted--;
        } else if (code === 'ArrowDown' && this.highlighted < this.results.length - 1) {
          this.highlighted++;
        } else if (code === 'Enter' && this.highlighted >= 0) {
          this.select(this.highlighted);
        } else if (code === 'Enter' && this.results.length > 0) {
          this.select(0);
        } else if (metaKey && digits.hasOwnProperty(code)) {
          this.select(digits[code]);
        }
      },
      select (index) {
        if (!this.results[index]) {
          return;
        }

        const { key, pluginName } = this.results[index];

        ipcRenderer.send('input:select?', pluginName, key);
      },
    },
  };
</script>
