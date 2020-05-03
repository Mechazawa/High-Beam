const webpack = require('webpack');

module.exports = {
  pluginOptions: {
    electronBuilder: {
      chainWebpackMainProcess: config => {
        config.module
          .rule('babel')
          .test(/(\.js$)/)
          .use('babel')
          .loader('babel-loader')
          .options(require('./babel.config.js'));
      },
      plugins: [
        new webpack.HotModuleReplacementPlugin(),
        new webpack.NamedModulesPlugin(),
      ],
    },
  },
};
