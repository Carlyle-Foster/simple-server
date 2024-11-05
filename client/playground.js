let canvas = document.getElementById("playground");
let ctx = canvas.getContext("2d");
ctx.fillStyle = "cyan";
for (let i = 0; i < 10; i++) {
	ctx.fillRect(60*i,0,50,50);
}