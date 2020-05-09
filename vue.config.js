const webpack = require('webpack');

module.exports = {
  pluginOptions: {
    electronBuilder: {
      builderOptions: {
        appId: 'at.ioexception.high-beam',
        productName: 'High Beam',
        mac: {
          darkModeSupport: true,
        },
      },
      chainWebpackMainProcess: config => {
        config.module
          .rule('babel')
          .test(/(\.js$)/i)
          .use('babel')
          .loader('babel-loader?cacheDirectory')
          .options(require('./babel.config.js'));

        config.module
          .rule('file-path-loader')
          .test(/\.(png|jpe?g|gif|svg)$/i)
          .use('file-path-loader')
          .loader('file-loader?outputPath=assets');
      },
      plugins: [
        new webpack.HotModuleReplacementPlugin(),
        new webpack.NamedModulesPlugin(),
      ],
    },
  },
};
