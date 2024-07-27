<template>
  <div class="row" :class="{ highlight: highlight }">
    <img v-if="icon" :src="icon" alt="icon" class="icon cell"/>
    <div v-else class="icon cell"/>
    <div class="cell">
      <template v-if="html">
        <strong v-html="title" class="cut-text"/>
        <span v-html="currentDescription" v-if="description" :class="['description' ,{ 'cut-text': !showExtended }]"/>
      </template>
      <template v-else>
        <strong v-text="title" class="cut-text"/>
        <span v-text="currentDescription" v-if="description" :class="['description' ,{ 'cut-text': !showExtended }]"/>
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
      descriptionExtended: {
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
    data () {
      return {
        mightShowExtended: false,
        boundOnKeydown: this.onKeydown.bind(this),
        boundKeyup: this.onKeyup.bind(this),
      };
    },
    mounted () {
      window.addEventListener('keydown', this.boundOnKeydown);
      window.addEventListener('keyup', this.boundKeyup);
    },
    beforeDestroy () {
      window.removeEventListener('keydown', this.boundOnKeydown);
      window.removeEventListener('keyup', this.boundKeyup);
    },
    computed: {
      showExtended () {
        return this.mightShowExtended && this.highlight;
      },
      currentDescription () {
        return this.showExtended && this.descriptionExtended ? this.descriptionExtended : this.description;
      },
    },
    methods: {
      onKeydown ({ code }) {
        if (code === 'MetaRight' || code === 'MetaLeft') {
          this.mightShowExtended = true;
        }
      },
      onKeyup ({ code }) {
        if (code === 'MetaRight' || code === 'MetaLeft') {
          this.mightShowExtended = false;
        }
      },
    },
  };
</script>

<style scoped lang="scss">
  @import '../assets/variables.scss';

  .row {
    height: 60px;
    width: 800px;
    display: table;

    :hover {
      cursor: pointer;
    }
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
    width: 50em;
    display: block;
    font-size: 10pt;
    color: var(--subtext-font-color);
  }

  .highlight {
    background: var(--background-color-highlight);

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

  .cut-text {
    text-overflow: ellipsis;
    overflow: hidden;
    white-space: nowrap;
    max-height: 1.5em;
  }
</style>
