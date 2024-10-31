module.exports = {
    mode: "jit",
    content: {
        files: ["*.html"],
    }, 
    plugins: [
        require('flowbite/plugin'),
        require('@tailwindcss/typography'),
    ],
}
