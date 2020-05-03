<template>
  <div>
    <QueryResultRow
        v-for="(result, index) in results"
        :key="result.key"
        :index="index"
        v-bind="result"
        :highlight="selected === index"
        @click="activate(index)"/>
  </div>
</template>

<script>
  import QueryResultRow from "./QueryResultRow";

  export default {
    name: 'query-result-table',
    components: { QueryResultRow },
    props: {
      results: Array,
    },
    data () {
      return {
        selected: -1,
        boundOnKeydown: (...args) => this.onKeydown(...args),
      };
    },
    watch: {
      results () {
        this.selected = -1;
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
        const digits = Object.fromEntries([1234567890].map(x => [`Digit${x}`, x]));

        if (code === 'ArrowUp' && this.selected >= 0) {
          this.selected--;
        } else if (code === 'ArrowDown' && this.selected < this.results.length - 1) {
          this.selected++;
        } else if (code === 'Enter' && this.selected >= 0) {
          this.activate(this.selected);
        } else if (metaKey && digits.hasOwnProperty(code)) {
          this.activate(digits[code]);
        }
      },
      activate (index) {

      },
    },
  };
</script>

<style scoped>

</style>
