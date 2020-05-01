<template>
  <div id="app">
    <!--suppress HtmlFormInputWithoutLabel -->
    <input placeholder="Query..." v-model="query" class="query" @keydown="onKeypress">
  </div>
</template>

<script>
  import { ipcRenderer } from 'electron'

  export default {
    name: 'app',
    data () {
      return {
        query: '',
        height: 80,
      };
    },
    methods: {
      onKeypress ({ code }) {
        if (code === 'Escape') {
          window.close();
        }

        this.height++;
        ipcRenderer.send('window:bounds?', { height: this.height });
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
    padding: 0;
    margin: 0;
    outline: 0;
    border: 0;

    :focus {
      outline: 0;
      border: 0;
    }
  }
</style>
