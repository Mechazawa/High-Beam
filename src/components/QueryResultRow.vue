<template>
  <div class="row" v-bind:class="{highlight: highlight}">
    <img v-if="icon" :src="icon" alt="icon" class="icon cell"/>
    <div v-else class="icon cell"/>
    <div class="cell">
      <strong v-text="title"/>
      <span v-text="description" v-if="description" class="description"/>
    </div>
    <div class="index cell" v-if="index >= 0" v-text="index + 1"/>
  </div>
</template>

<script>
  export default {
    name: 'query-result-row',
    props: {
      title: {
        type: String,
        required: true,
      },
      icon: {
        type: String,
        default: '',
      },
      description: {
        type: String,
        default: '',
      },
      index: {
        type: Number,
        default: -1,
      },
      highlight: {
        type: Boolean,
      },
    },
  };
</script>

<style scoped lang="scss">
  .row {
    height: 60px;
    display: table;
  }

  .icon {
    height: calc(60px - 1em);
    width: calc(60px - 1em);

    padding: .5em;
    float: left;
  }

  .cell {
    display: table-cell;
    vertical-align: middle;
  }

  div {
    width: 100%;

    > * {
      margin: 0;
    }
  }

  strong {
    font-family: Avenir, Helvetica, Arial, sans-serif;
    -webkit-font-smoothing: antialiased;
    -moz-osx-font-smoothing: grayscale;
    color: #2c3e50;
    display: block;
    font-size: 14pt;
  }

  .description {
    font-size: 10pt;
    color: #929292;
  }

  :hover, .highlight {
    background: gray;
    cursor: pointer;

    > strong {
      color: #efefef;
    }

    > .description {
      color: #c7c7c7;
    }
  }

  .index::before {
    content: '⌘'
  }

  .index {
    float: right;
    font-size: 30pt;
    padding-right: .2em;
  }
</style>
