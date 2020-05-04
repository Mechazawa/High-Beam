<template>
  <div class="row" :class="{ highlight: highlight }">
    <img v-if="icon" :src="icon" alt="icon" class="icon cell"/>
    <div v-else class="icon cell"/>
    <div class="cell">
      <template v-if="html">
        <strong v-html="title"/>
        <span v-html="description" v-if="description" class="description"/>
      </template>
      <template v-else>
        <strong v-text="title"/>
        <span v-text="description" v-if="description" class="description"/>
      </template>
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
      html: {
        type: Boolean,
      },
    },
  };
</script>

<style scoped lang="scss">
  @import '../assets/variables.scss';

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
    color: var(--main-font-color);
    display: block;
    font-size: 14pt;
  }

  .description {
    font-size: 10pt;
    color: var(--subtext-font-color);
  }

  .row:hover, .highlight {
    background: var(--background-color-highlight);
    cursor: pointer;

    strong, .index {
      color: var(--main-font-color-highlight);
    }

    .description {
      color: var(--subtext-font-color-highlight);
    }
  }

  .index::before {
    content: '⌘'
  }

  .index {
    font-size: 24pt;
    padding-right: .5em;
    color: var(--main-font-color);
  }
</style>
